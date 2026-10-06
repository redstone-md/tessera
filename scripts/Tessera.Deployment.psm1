# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
#
# Shared deployment and recovery engine for the Tessera shell on Windows 11
# x64 client editions (Home/Pro). Compatible with Windows PowerShell 5.1 and
# PowerShell 7; uses standard .NET and Windows built-ins only.
#
# Consumers: scripts/Install-Tessera.ps1 and the independent copy of
# scripts/Restore-Tessera.ps1 that ships next to this module in the stable
# recovery directory. The restore copy must keep working with no Tessera
# binaries present, so this module never launches or requires them.
#
# The engine derives the shell command exclusively from the installed
# package location. It never accepts a persisted arbitrary executable
# command, never deletes user data, and never removes the Winlogon key.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Win32 registry type codes used by the shared recovery contract
# (REG_SZ = 1, REG_EXPAND_SZ = 2), not the .NET RegistryValueKind numbers.
$script:RegistryTypeSz = 1
$script:RegistryTypeExpandSz = 2
$script:ShellValueName = 'Shell'
$script:RecoverySchemaVersion = 1
# The only version form the immutable install directory accepts; this also
# makes the version safe as a single directory name (no traversal).
$script:AlphaVersionPattern = '^\d+\.\d+\.\d+-alpha\.\d+$'
$script:RequiredRecoveryFields = @(
    'SchemaVersion', 'Active', 'ShellCommand', 'OriginalPresent',
    'OriginalKind', 'OriginalValue', 'InstallDirectory'
)

# Module-internal seam. Production code never mutates it; the focused test
# script replaces the hive handles, the local-app-data root, and the domain
# probe with temporary fixtures so no test touches the real Winlogon paths.
$script:DeploymentContext = @{
    UserHive = $null
    MachineHive = $null
    WinlogonSubKey = 'SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
    PolicySystemSubKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
    SmartAppControlPolicySubKey = 'SYSTEM\CurrentControlSet\Control\CI\Policy'
    RecoverySubKey = 'SOFTWARE\Tessera\ShellRecovery'
    OsSubKey = 'SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    LocalAppDataRoot = [Environment]::GetFolderPath('LocalApplicationData')
    DomainProbe = $null
    LockRetryCount = 25
    LockRetryMilliseconds = 200
    Hooks = @{}
}

$script:DryRun = $false

function Get-DeploymentUserHive {
    if ($null -eq $script:DeploymentContext.UserHive) {
        $script:DeploymentContext.UserHive = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
            [Microsoft.Win32.RegistryHive]::CurrentUser,
            [Microsoft.Win32.RegistryView]::Registry64)
    }
    return $script:DeploymentContext.UserHive
}

function Get-DeploymentMachineHive {
    if ($null -eq $script:DeploymentContext.MachineHive) {
        $script:DeploymentContext.MachineHive = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
            [Microsoft.Win32.RegistryHive]::LocalMachine,
            [Microsoft.Win32.RegistryView]::Registry64)
    }
    return $script:DeploymentContext.MachineHive
}

function Open-DeploymentRegistryKey {
    param(
        [Parameter(Mandatory)][ValidateSet('User', 'Machine')][string]$Scope,
        [Parameter(Mandatory)][string]$SubKey,
        [switch]$Writable
    )
    $hive = if ($Scope -eq 'User') { Get-DeploymentUserHive } else { Get-DeploymentMachineHive }
    # A WhatIf run must not create keys either; read-only handles still let
    # the engine run its full checks without persisting anything.
    if ($Writable -and -not $script:DryRun) {
        return $hive.CreateSubKey($SubKey, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree)
    }
    return $hive.OpenSubKey($SubKey, $false)
}

# Reads a value without expanding REG_EXPAND_SZ data. Kind is $null when the
# value is absent; Value is $null for kinds this module does not manage.
function Get-DeploymentRawValue {
    param(
        [Parameter(Mandatory)][Microsoft.Win32.RegistryKey]$Key,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Name
    )
    if (@($Key.GetValueNames()) -notcontains $Name) {
        return @{ Present = $false; Value = $null; Kind = $null }
    }
    $kind = $Key.GetValueKind($Name)
    if ($kind -eq [Microsoft.Win32.RegistryValueKind]::String -or
        $kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) {
        $value = [string]$Key.GetValue($Name, '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    } else {
        $value = $null
    }
    return @{ Present = $true; Value = $value; Kind = $kind }
}

# Maps a managed raw-value record to the recovery-contract type code.
function ConvertTo-RegistryTypeCode {
    param([Parameter(Mandatory)][Microsoft.Win32.RegistryValueKind]$Kind)
    if ($Kind -eq [Microsoft.Win32.RegistryValueKind]::String) { return $script:RegistryTypeSz }
    if ($Kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) { return $script:RegistryTypeExpandSz }
    throw "Unsupported registry value kind: $Kind"
}

function ConvertFrom-RegistryTypeCode {
    param([Parameter(Mandatory)][int]$TypeCode)
    if ($TypeCode -eq $script:RegistryTypeSz) { return [Microsoft.Win32.RegistryValueKind]::String }
    if ($TypeCode -eq $script:RegistryTypeExpandSz) { return [Microsoft.Win32.RegistryValueKind]::ExpandString }
    return $null
}

function Get-DeploymentLocalAppDataRoot {
    $root = $script:DeploymentContext.LocalAppDataRoot
    if ([string]::IsNullOrWhiteSpace($root)) {
        throw 'The per-user local application data folder could not be resolved.'
    }
    return $root
}

function Get-TesseraInstallRoot {
    return Join-Path (Get-DeploymentLocalAppDataRoot) 'Programs\Tessera'
}

function Get-TesseraRecoveryDirectory {
    return Join-Path (Get-DeploymentLocalAppDataRoot) 'Tessera\Recovery'
}

function Get-DeploymentLockPath {
    return Join-Path (Get-DeploymentLocalAppDataRoot) 'Tessera\deployment.lock'
}

# Serializes every modifying deployment/recovery path with an exclusive file
# lock; the native recovery runtime waits on the same file with share mode 0.
# The caller owns the returned stream and must dispose it.
function New-DeploymentLock {
    [CmdletBinding()]
    param()

    $lockPath = Get-DeploymentLockPath
    $lockParent = Split-Path -Parent $lockPath
    if (-not (Test-Path -LiteralPath $lockParent -PathType Container)) {
        New-Item -ItemType Directory -Path $lockParent -Force | Out-Null
    }
    $attempts = [int]$script:DeploymentContext.LockRetryCount
    $delay = [int]$script:DeploymentContext.LockRetryMilliseconds
    for ($attempt = 1; $attempt -le $attempts; $attempt++) {
        try {
            return [System.IO.FileStream]::new(
                $lockPath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None)
        } catch [System.IO.IOException] {
            if ($attempt -eq $attempts) { break }
            Start-Sleep -Milliseconds $delay
        }
    }
    throw "The Tessera deployment lock is held by another process: $lockPath"
}

# Static, side-effect-free package validation: completeness, PE x64
# architecture of the shipped executables, and the BUILD-INFO version.
function Test-TesseraPackage {
    [CmdletBinding()][OutputType([pscustomobject])]
    param([Parameter(Mandatory)][string]$PackagePath)

    if (-not (Test-Path -LiteralPath $PackagePath -PathType Container)) {
        throw "Package path is not an existing directory: $PackagePath"
    }
    $packagePath = [IO.Path]::GetFullPath($PackagePath)
    $required = 'Tessera.exe', 'tessera-cli.exe', 'tessera-shell.exe', 'BUILD-INFO.txt'
    foreach ($name in $required) {
        $candidate = Join-Path $packagePath $name
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Package is incomplete; missing file: $name"
        }
    }
    foreach ($name in 'Tessera.exe', 'tessera-cli.exe', 'tessera-shell.exe') {
        if (-not (Test-DeploymentPeX64 -Path (Join-Path $packagePath $name))) {
            throw "Package executable is not a 64-bit x64 PE image: $name"
        }
    }
    foreach ($name in 'LICENSE.txt', 'START-HERE.txt', 'THIRD-PARTY-NOTICES.txt') {
        $candidate = Join-Path $packagePath $name
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Package is incomplete; missing documentation file: $name"
        }
    }
    foreach ($name in 'Install-Tessera.ps1', 'Restore-Tessera.ps1', 'Tessera.Deployment.psm1') {
        $candidate = Join-Path $packagePath $name
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Package is incomplete; missing deployment script: $name"
        }
    }

    $buildInfo = Get-DeploymentBuildInfo -Path (Join-Path $packagePath 'BUILD-INFO.txt')
    return [pscustomobject]@{
        PackagePath = $packagePath
        Version = $buildInfo.Version
        Commit = $buildInfo.Commit
        SupervisorPath = Join-Path $packagePath 'tessera-shell.exe'
    }
}

function Get-DeploymentBuildInfo {
    param([Parameter(Mandatory)][string]$Path)
    # Exact BUILD-INFO.txt format produced by the packaging script:
    # first line 'Tessera <version>' (an optional leading 'v' is accepted),
    # second line 'Source commit: <sha>', then informational lines. There
    # is no 'Version:' key. The version must be a numbered alpha so it can
    # never escape the immutable version directory as a traversal or
    # arbitrary name.
    $version = $null
    $commit = $null
    foreach ($line in (Get-Content -LiteralPath $Path)) {
        if ($null -eq $version -and $line -match '^\s*Tessera\s+(v?\S+)\s*$') {
            $version = $Matches[1]
        } elseif ($null -eq $commit -and $line -match '^\s*Source commit\s*:\s*([0-9a-fA-F]{40})\s*$') {
            $commit = $Matches[1]
        }
    }
    if ($null -ne $version -and $version.StartsWith('v')) { $version = $version.Substring(1) }
    if ($null -eq $version -or $version -notmatch $script:AlphaVersionPattern) {
        throw ("BUILD-INFO.txt does not declare a numbered alpha version (for example 'Tessera 0.1.0-alpha.2'): " + $Path)
    }
    if ([string]::IsNullOrWhiteSpace($commit)) {
        throw "BUILD-INFO.txt does not declare a 'Source commit: <40-hex>' line: $Path"
    }
    return @{ Version = $version; Commit = $commit }
}

function Test-DeploymentPeX64 {
    param([Parameter(Mandatory)][string]$Path)
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 0x46 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { return $false }
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($peOffset -lt 0 -or ($peOffset + 6) -gt $bytes.Length) { return $false }
    if ([BitConverter]::ToUInt32($bytes, $peOffset) -ne 0x00004550) { return $false }
    return [BitConverter]::ToUInt16($bytes, $peOffset + 4) -eq 0x8664
}

# Activation preflight. Read-only by design; every problem reported here
# means the caller must not touch the shell configuration at all.
function Test-ShellSupport {
    [CmdletBinding()][OutputType([pscustomobject])]
    param()

    $problems = @()

    if ([IntPtr]::Size -ne 8) {
        $problems += 'A 64-bit (x64) PowerShell process is required.'
    }
    $problems += Get-DeploymentOsProblems
    $problems += Get-DeploymentDomainProblems
    $problems += Get-DeploymentPolicyProblems
    $problems += Get-DeploymentShellProblems
    $problems += Get-DeploymentSmartAppControlProblems

    return [pscustomobject]@{
        Supported = ($problems.Count -eq 0)
        Problems = $problems
    }
}

# Smart App Control can block unsigned executables before the supervisor
# ever runs. Per Microsoft's documented registry values
# (VerifiedAndReputablePolicyState): 0 = Off, 1 = On (Enforcement),
# 2 = Evaluation. A known enforcing state refuses the unsigned shell
# activation; a state that cannot be verified is refused conservatively
# rather than treated as approval; Off/Evaluation still require the
# supervisor probe because policy can change at the next sign-in.
function Get-DeploymentSmartAppControlProblems {
    $key = Open-DeploymentRegistryKey -Scope Machine -SubKey $script:DeploymentContext.SmartAppControlPolicySubKey
    if ($null -eq $key) {
        return @('The Smart App Control policy key could not be read; the shell activation is refused because the state cannot be verified.')
    }
    try {
        $raw = Get-DeploymentRawValue -Key $key -Name 'VerifiedAndReputablePolicyState'
        if (-not $raw.Present -or $raw.Kind -ne [Microsoft.Win32.RegistryValueKind]::DWord) {
            return @('The Smart App Control policy state could not be read; the shell activation is refused because the state cannot be verified.')
        }
        # The shared raw shell-value reader deliberately exposes strings only.
        # Read this verified DWORD as its native integer, never parse REG_SZ.
        $state = [int]$key.GetValue('VerifiedAndReputablePolicyState')
        switch ($state) {
            0 { return @() }    # Off: documented, probe still mandatory.
            1 {
                return @('Smart App Control is enforcing (state 1); it is known to block unsigned executables, so the unsigned shell activation is refused.')
            }
            2 { return @() }    # Evaluation: documented, probe still mandatory.
            default {
                return @("The Smart App Control policy state $state is not a documented value; the shell activation is refused because the state cannot be verified.")
            }
        }
    } finally {
        if ($null -ne $key) { $key.Dispose() }
    }
}

function Get-DeploymentOsProblems {
    $key = Open-DeploymentRegistryKey -Scope Machine -SubKey $script:DeploymentContext.OsSubKey
    if ($null -eq $key) { return @('The Windows version could not be read from the registry.') }
    try {
        $problems = @()
        $buildRaw = Get-DeploymentRawValue -Key $key -Name 'CurrentBuildNumber'
        if (-not $buildRaw.Present -or $null -eq $buildRaw.Value) {
            return @('The Windows build number could not be read from the registry.')
        }
        $build = 0
        if (-not [int]::TryParse($buildRaw.Value.Trim(), [ref]$build)) {
            return @('The Windows build number is not readable: ' + $buildRaw.Value)
        }
        if ($build -lt 22000) {
            $problems += "Windows 11 (build 22000 or later) is required; this host reports build $build."
        }
        $typeRaw = Get-DeploymentRawValue -Key $key -Name 'InstallationType'
        if (-not $typeRaw.Present -or $null -eq $typeRaw.Value) {
            $problems += 'The Windows installation type could not be read from the registry.'
        } elseif ($typeRaw.Value.Trim() -ne 'Client') {
            $problems += "Only Windows client editions are supported; this host reports installation type '$($typeRaw.Value.Trim())'."
        }
        return $problems
    } finally {
        if ($null -ne $key) { $key.Dispose() }
    }
}

function Get-DeploymentDomainProblems {
    try {
        $probe = $script:DeploymentContext.DomainProbe
        $joined = if ($null -eq $probe) {
            [bool](Get-CimInstance -ClassName Win32_ComputerSystem -Property PartOfDomain).PartOfDomain
        } else {
            [bool](& $probe)
        }
        if ($joined) {
            return @('Domain-joined hosts are not supported for the experimental per-user shell override.')
        }
        return @()
    } catch {
        return @("Domain membership could not be verified; refusing shell activation. $($_.Exception.Message)")
    }
}

function Get-DeploymentPolicyProblems {
    $problems = @()
    foreach ($scope in 'User', 'Machine') {
        $key = Open-DeploymentRegistryKey -Scope $scope -SubKey $script:DeploymentContext.PolicySystemSubKey
        if ($null -eq $key) { continue }
        try {
            $raw = Get-DeploymentRawValue -Key $key -Name $script:ShellValueName
            if ($raw.Present) {
                $problems += "A policy-managed Shell value exists in the $scope Policies\System key; shell activation is refused."
            }
        } finally {
            if ($null -ne $key) { $key.Dispose() }
        }
    }
    return $problems
}

function Get-DeploymentShellProblems {
    $problems = @()
    $machine = Open-DeploymentRegistryKey -Scope Machine -SubKey $script:DeploymentContext.WinlogonSubKey
    if ($null -eq $machine) {
        return @('The machine Winlogon key could not be read.')
    }
    try {
        $raw = Get-DeploymentRawValue -Key $machine -Name $script:ShellValueName
        if ($raw.Present -and -not (Test-DeploymentExplorerCommand -Raw $raw)) {
            $problems += 'The machine Winlogon Shell value is not Explorer; shell activation is refused.'
        }
    } finally {
        if ($null -ne $machine) { $machine.Dispose() }
    }

    $user = Open-DeploymentRegistryKey -Scope User -SubKey $script:DeploymentContext.WinlogonSubKey
    if ($null -eq $user) { return $problems }
    try {
        $raw = Get-DeploymentRawValue -Key $user -Name $script:ShellValueName
        if (-not $raw.Present) { return $problems }
        if ($null -eq $raw.Value) {
            $problems += "The per-user Winlogon Shell value has an unsupported registry type ($($raw.Kind)); shell activation is refused."
        } elseif (Test-DeploymentExplorerCommand -Raw $raw) {
            return $problems
        } elseif (Test-DeploymentOwnShellCommand -Raw $raw -Kind $raw.Kind) {
            return $problems
        } else {
            $problems += "The per-user Winlogon Shell value is already set to another program; shell activation is refused."
        }
    } finally {
        if ($null -ne $user) { $user.Dispose() }
    }
    return $problems
}

function Test-DeploymentExplorerCommand {
    param([Parameter(Mandatory)]$Raw)
    if (-not $Raw.Present -or $null -eq $Raw.Value) { return $false }
    $expanded = $Raw.Value
    if ($Raw.Kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) {
        $expanded = [Environment]::ExpandEnvironmentVariables($Raw.Value)
    }
    $literal = $expanded.Trim().Trim('"')
    if ([string]::Equals($literal, 'explorer.exe', [StringComparison]::OrdinalIgnoreCase)) { return $true }
    # An arbitrary third-party executable named explorer.exe is not the
    # Windows shell. Accept only the actual system directory, without args.
    $windows = [Environment]::GetFolderPath('Windows')
    if ([string]::IsNullOrWhiteSpace($windows) -or -not [IO.Path]::IsPathRooted($literal)) { return $false }
    try {
        return [string]::Equals([IO.Path]::GetFullPath($literal),
            (Join-Path $windows 'explorer.exe'), [StringComparison]::OrdinalIgnoreCase)
    } catch { return $false }
}

# True when the current per-user Shell value is exactly the command this
# deployment previously recorded and the backup is still active, so
# re-running preflight stays idempotent. Ownership is typed: the value must
# be a present REG_SZ whose bytes equal the recorded command; a foreign
# REG_EXPAND_SZ with identical bytes is not accepted.
function Test-DeploymentOwnShellCommand {
    param(
        [Parameter(Mandatory)]$Raw,
        [Parameter(Mandatory)][AllowNull()][Microsoft.Win32.RegistryValueKind]$Kind
    )
    if (-not $Raw.Present -or $null -eq $Raw.Value) { return $false }
    $state = Get-ShellRecoveryState
    if (-not $state.Exists -or -not $state.Active) { return $false }
    if (-not $state.Fields.ContainsKey('ShellCommand')) { return $false }
    $shellCommand = [string]$state.Fields['ShellCommand']
    if ([string]::IsNullOrWhiteSpace($shellCommand)) { return $false }
    return ($Kind -eq [Microsoft.Win32.RegistryValueKind]::String -and
        [string]::Equals($Raw.Value, $shellCommand, [StringComparison]::Ordinal))
}

# Decode helper: strict fail-closed read of one recovery field. Fields must
# exist; DWORD counters (SchemaVersion, Active, OriginalPresent, Kind) must
# be REG_DWORD with an in-range non-negative value; string fields must be
# REG_SZ. No normalization, no coercion, and no unbounded string data.
function Get-DeploymentRecoveryField {
    param(
        [Parameter(Mandatory)][Microsoft.Win32.RegistryKey]$Key,
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][bool]$IsCounter
    )
    $raw = Get-DeploymentRawValue -Key $Key -Name $Name
    if (-not $raw.Present) {
        throw "The recovery field $Name is missing from the backup."
    }
    if ($IsCounter) {
        if ($raw.Kind -ne [Microsoft.Win32.RegistryValueKind]::DWord) {
            throw "The recovery field $Name is not a REG_DWORD value."
        }
        $value = [int]$Key.GetValue($Name)
        if ($value -lt 0) {
            throw "The recovery field $Name is negative and cannot be decoded."
        }
        return $value
    }
    if ($raw.Kind -ne [Microsoft.Win32.RegistryValueKind]::String) {
        throw "The recovery field $Name is not a REG_SZ value."
    }
    $value = [string]$raw.Value
    if ($value.Length -ge 32768 -or $value.Contains([char]0)) {
        throw "The recovery field $Name is not a bounded, NUL-free UTF-16 string."
    }
    # Refuse unpaired surrogates rather than replacing the stored data.
    $null = [Text.UTF8Encoding]::new($false, $true).GetByteCount($value)
    return $value
}

function Get-ShellRecoveryState {
    [CmdletBinding()][OutputType([pscustomobject])]
    param()

    $key = Open-DeploymentRegistryKey -Scope User -SubKey $script:DeploymentContext.RecoverySubKey
    if ($null -eq $key) {
        return [pscustomobject]@{ Exists = $false; Active = $false; Fields = @{} }
    }
    try {
        if (@($key.GetValueNames()).Count -eq 0) {
            return [pscustomobject]@{ Exists = $false; Active = $false; Fields = @{} }
        }
        $fields = @{}
        foreach ($name in $script:RequiredRecoveryFields) {
            $isCounter = $name -in 'SchemaVersion', 'Active', 'OriginalPresent', 'OriginalKind'
            $fields[$name] = Get-DeploymentRecoveryField -Key $key -Name $name -IsCounter $isCounter
        }
        if ([int]$fields['SchemaVersion'] -ne $script:RecoverySchemaVersion) {
            throw "The recovery backup uses schema version $($fields['SchemaVersion']); only schema $script:RecoverySchemaVersion is supported."
        }
        if ([int]$fields['OriginalPresent'] -ne 0 -and [int]$fields['OriginalPresent'] -ne 1) {
            throw "The recovery field OriginalPresent must be 0 or 1; found $($fields['OriginalPresent'])."
        }
        if ([int]$fields['Active'] -ne 0 -and [int]$fields['Active'] -ne 1) {
            throw "The recovery field Active must be 0 or 1; found $($fields['Active'])."
        }
        if ([int]$fields['OriginalPresent'] -eq 1) {
            $originalKind = [int]$fields['OriginalKind']
            if ($null -eq (ConvertFrom-RegistryTypeCode -TypeCode $originalKind)) {
                throw "The recovery field OriginalKind is not a supported registry type: $originalKind."
            }
        }
        elseif ([int]$fields['OriginalKind'] -ne 1 -or [string]$fields['OriginalValue'] -ne '') {
            throw 'The recovery backup has inconsistent absent-original fields.'
        }
        $directory = [string]$fields['InstallDirectory']
        $command = [string]$fields['ShellCommand']
        if ($directory -notmatch '^(?:[A-Za-z]:[\\/]|\\\\)' -or
            $directory -match '[\x00-\x1f"]' -or
            $command.Length -lt 3 -or -not $command.StartsWith('"') -or -not $command.EndsWith('"')) {
            throw 'The recovery backup does not name an absolute installed supervisor.'
        }
        $executable = $command.Substring(1, $command.Length - 2)
        $separator = $executable.LastIndexOfAny([char[]]@('\', '/'))
        if ($separator -lt 1 -or $executable -match '[\x00-\x1f"]' -or
            -not [string]::Equals($executable.Substring(0, $separator), $directory.TrimEnd([char[]]@('\', '/')), [StringComparison]::Ordinal) -or
            -not [string]::Equals($executable.Substring($separator + 1), 'tessera-shell.exe', [StringComparison]::OrdinalIgnoreCase)) {
            throw 'The recovery command must name only tessera-shell.exe in the recorded installation directory.'
        }
        $active = [int]$fields['Active'] -eq 1
        return [pscustomobject]@{
            Exists = $true
            Active = $active
            Fields = $fields
        }
    } finally {
        if ($null -ne $key) { $key.Dispose() }
    }
}

# Copies the package into an immutable per-version directory under
# LOCALAPPDATA\Programs\Tessera. A rerun with byte-identical content is
# idempotent; different content for the same version is refused because the
# directory is immutable and the installed binaries may be the active shell.
function Copy-TesseraPackage {
    [CmdletBinding(SupportsShouldProcess)][OutputType([string])]
    param(
        [Parameter(Mandatory)][string]$PackagePath,
        [Parameter(Mandatory)][string]$Version
    )

    $previousDryRun = $script:DryRun
    $script:DryRun = [bool]$WhatIfPreference
    try {
        $installRoot = Get-TesseraInstallRoot
        $final = [IO.Path]::GetFullPath((Join-Path $installRoot $Version))
        if (Test-Path -LiteralPath $final -PathType Container) {
            if ($script:DryRun) {
                # A plan must not stage a real copy just to compare manifests;
                # report the idempotent path without writing anything.
                Write-Host "WHATIF: An installed copy of version $Version already exists; would verify its content and copy nothing."
                return $final
            }
            $staged = Copy-DeploymentPackageToStaging -PackagePath $PackagePath -StagingRoot $installRoot
            try {
                if (Test-DeploymentSameManifest -Reference $staged -Other $final) {
                    Write-Verbose "Installed package $final already matches; nothing to copy."
                    return $final
                }
                throw ("An installed copy of version $Version already exists with different content; " +
                    "the version directory is immutable. Restore first if it is the active shell, " +
                    "then remove $final manually before reinstalling.")
            } finally {
                Remove-DeploymentDirectory -Path $staged
            }
        }

        if ($script:DryRun) {
            Write-Host "WHATIF: Would install package to $final"
            return $final
        }
        if (-not (Test-Path -LiteralPath $installRoot -PathType Container)) {
            New-Item -ItemType Directory -Path $installRoot -Force | Out-Null
        }
        $staged = Copy-DeploymentPackageToStaging -PackagePath $PackagePath -StagingRoot $installRoot
        try {
            [IO.Directory]::Move($staged, $final)
        } catch {
            Remove-DeploymentDirectory -Path $staged
            throw
        }
        Write-Verbose "Installed package to $final"
        return $final
    } finally {
        $script:DryRun = $previousDryRun
    }
}

function Copy-DeploymentPackageToStaging {
    param(
        [Parameter(Mandatory)][string]$PackagePath,
        [Parameter(Mandatory)][string]$StagingRoot
    )
    $staging = Join-Path $StagingRoot ('.staging-' + [Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($staging) | Out-Null
    foreach ($file in [IO.Directory]::EnumerateFiles($PackagePath, '*', [IO.SearchOption]::AllDirectories)) {
        $relative = $file.Substring($PackagePath.Length).TrimStart('\', '/')
        $target = Join-Path $staging $relative
        $targetParent = Split-Path -Parent $target
        if (-not [IO.Directory]::Exists($targetParent)) { [IO.Directory]::CreateDirectory($targetParent) | Out-Null }
        [IO.File]::Copy($file, $target, $true)
    }
    return $staging
}

function Get-DeploymentFileManifest {
    param([Parameter(Mandatory)][string]$Root)
    $rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $manifest = @{}
    foreach ($file in [IO.Directory]::EnumerateFiles($rootPath, '*', [IO.SearchOption]::AllDirectories)) {
        $relative = $file.Substring($rootPath.Length).TrimStart('\', '/')
        $manifest[$relative.ToLowerInvariant()] = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
    }
    return $manifest
}

function Test-DeploymentSameManifest {
    param(
        [Parameter(Mandatory)][string]$Reference,
        [Parameter(Mandatory)][string]$Other
    )
    # Distinct local names: reusing a case-variant of a parameter name breaks
    # the assignment in PowerShell 7.6 (the parameter value survives), which
    # made every manifest comparison see strings instead of hashtables.
    $referenceManifest = Get-DeploymentFileManifest -Root $Reference
    $otherManifest = Get-DeploymentFileManifest -Root $Other
    if ($referenceManifest.Count -ne $otherManifest.Count) { return $false }
    foreach ($entry in $referenceManifest.GetEnumerator()) {
        if (-not $otherManifest.ContainsKey($entry.Key)) { return $false }
        if ($otherManifest[$entry.Key] -ne $entry.Value) { return $false }
    }
    return $true
}

function Remove-DeploymentDirectory {
    param([Parameter(Mandatory)][string]$Path)
    if (Test-Path -LiteralPath $Path -PathType Container) {
        [IO.Directory]::Delete($Path, $true)
    }
}

# Publishes the independent restore script and this module into the stable
# recovery directory before any shell activation, verifying byte equality.
function Publish-RecoveryRuntime {
    [CmdletBinding(SupportsShouldProcess)]
    param([Parameter(Mandatory)][string]$SourceDirectory)

    $previousDryRun = $script:DryRun
    $script:DryRun = [bool]$WhatIfPreference
    try {
        if ($script:DryRun) {
            Write-Host "WHATIF: Would publish recovery scripts to $(Get-TesseraRecoveryDirectory)"
            return
        }
        $target = Get-TesseraRecoveryDirectory
        if (-not (Test-Path -LiteralPath $target -PathType Container)) {
            New-Item -ItemType Directory -Path $target -Force | Out-Null
        }
        foreach ($name in 'Restore-Tessera.ps1', 'Tessera.Deployment.psm1') {
            $source = Join-Path $SourceDirectory $name
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
                throw "Recovery runtime source file is missing: $source"
            }
            $destination = Join-Path $target $name
            [IO.File]::Copy($source, $destination, $true)
            if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne
                (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash) {
                Remove-Item -LiteralPath $destination -Force
                throw "The published recovery copy of $name does not match the source bytes; activation is refused."
            }
        }
        Write-Verbose "Published recovery runtime to $target"
    } finally {
        $script:DryRun = $previousDryRun
    }
}

# Backs up the current per-user Winlogon Shell value, verifies the backup by
# readback, publishes Active=1, and only then overrides Shell with the
# quoted supervisor command. Any failure after activation rolls the shell
# value and the Active flag back without touching unrelated values.
# The BeforeShellOverride hook (test-only) runs inside the same owned
# region as the override, so an injected failure exercises exactly the
# production rollback path.
#
# The caller must pass -PreflightPassed after running Test-ShellSupport and
# the supervisor runtime verification; the engine itself performs no
# process launches. Test fixtures bypass the runtime probe only through the
# module-internal Hooks seam, never through a public safety switch.
function Invoke-ShellActivation {
    [CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
    param(
        [Parameter(Mandatory)][string]$InstallDirectory,
        [switch]$PreflightPassed
    )

    if (-not $PreflightPassed) {
        throw 'Shell activation requires a completed preflight (Test-ShellSupport and the supervisor runtime verification); call it only after both succeeded.'
    }

    $previousDryRun = $script:DryRun
    $script:DryRun = [bool]$WhatIfPreference
    try {
        $supervisor = [IO.Path]::GetFullPath((Join-Path $InstallDirectory 'tessera-shell.exe'))
        if (-not (Test-Path -LiteralPath $supervisor -PathType Leaf)) {
            throw "The installed shell supervisor is missing: $supervisor"
        }
        $shellCommand = '"' + $supervisor + '"'
        $installDirectory = [IO.Path]::GetFullPath($InstallDirectory)

        $winlogon = Open-DeploymentRegistryKey -Scope User -SubKey $script:DeploymentContext.WinlogonSubKey -Writable
        if ($null -eq $winlogon) {
            throw 'The per-user Winlogon key could not be opened for activation.'
        }
        try {
            $current = Get-DeploymentRawValue -Key $winlogon -Name $script:ShellValueName
            if ($current.Present -and $null -eq $current.Value) {
                throw "The existing Shell value has registry type $($current.Kind), which this deployment does not manage."
            }
            $original = Resolve-DeploymentActivationBackup -Current $current -ShellCommand $shellCommand -InstallDirectory $installDirectory
            $recovery = Open-DeploymentRegistryKey -Scope User -SubKey $script:DeploymentContext.RecoverySubKey -Writable
            try {
                Write-DeploymentRecoveryFields -Key $recovery -ShellCommand $shellCommand -InstallDirectory $installDirectory -Original $original
                if (-not $script:DryRun) {
                    $backup = Read-DeploymentBackupKey
                    if ([int]$backup.Fields['OriginalPresent'] -ne [int]$original.Present -or
                        [int]$backup.Fields['OriginalKind'] -ne [int]$original.Kind -or
                        -not [string]::Equals($backup.Fields['OriginalValue'], $original.Value, [StringComparison]::Ordinal) -or
                        -not [string]::Equals($backup.Fields['ShellCommand'], $shellCommand, [StringComparison]::Ordinal) -or
                        -not [string]::Equals($backup.Fields['InstallDirectory'], $installDirectory, [StringComparison]::Ordinal)) {
                        throw 'The recovery backup does not match the original and deployment identity after writing.'
                    }
                }
                Set-DeploymentActiveState -Key $recovery -Active $true

                if ($script:DryRun) {
                    Write-Host "WHATIF: Would set the per-user Winlogon Shell value to $shellCommand"
                    return
                }

                $overrideError = $null
                $rollbackError = $null
                try {
                    if ($script:DeploymentContext.Hooks.ContainsKey('BeforeShellOverride')) {
                        # Test hook: injects a failure inside the same owned
                        # override region, so an injected fault rolls the
                        # shell value and the Active flag back exactly like
                        # a real write failure.
                        & $script:DeploymentContext.Hooks['BeforeShellOverride']
                    }
                    $winlogon.SetValue($script:ShellValueName, $shellCommand, [Microsoft.Win32.RegistryValueKind]::String)
                    $after = Get-DeploymentRawValue -Key $winlogon -Name $script:ShellValueName
                    if (-not $after.Present -or $after.Kind -ne [Microsoft.Win32.RegistryValueKind]::String -or
                        -not [string]::Equals($after.Value, $shellCommand, [StringComparison]::Ordinal)) {
                        throw 'the new Shell value did not verify after being written'
                    }
                } catch {
                    $overrideError = $_.Exception.Message
                    try {
                        Restore-DeploymentShellValue -Winlogon $winlogon -Original $original
                        Set-DeploymentActiveState -Key $recovery -Active $false
                    } catch {
                        $rollbackError = $_.Exception.Message
                    }
                }
                if ($overrideError) {
                    $message = "Setting the Shell value failed: $overrideError"
                    if ($rollbackError) { $message += " Rolling the override back also failed: $rollbackError" }
                    throw $message
                }
                Write-Verbose "The per-user shell now starts $shellCommand at the next sign-in."
            } finally {
                if ($null -ne $recovery) { $recovery.Dispose() }
            }
        } finally {
            if ($null -ne $winlogon) { $winlogon.Dispose() }
        }
    } finally {
        $script:DryRun = $previousDryRun
    }
}

# Decides the Original* backup fields before opening a writable recovery key.
# A truly empty key is no record; incomplete or unknown nonempty records are
# refused by the decoder. Preserve the original across idempotent attempts.
function Resolve-DeploymentActivationBackup {
    param(
        [Parameter(Mandatory)]$Current,
        [Parameter(Mandatory)][string]$ShellCommand,
        [Parameter(Mandatory)][string]$InstallDirectory
    )
    # State was decoded read-only before any recovery key creation.
    $existing = Read-DeploymentBackupKey
    if (-not $existing.Exists) {
        return @{
            Present = [bool]$Current.Present
            Kind = if ($Current.Present) { ConvertTo-RegistryTypeCode -Kind $Current.Kind } else { $script:RegistryTypeSz }
            Value = if ($Current.Present) { [string]$Current.Value } else { '' }
        }
    }
    if ([int]$existing.Fields['SchemaVersion'] -ne $script:RecoverySchemaVersion) {
        throw "The existing recovery backup uses schema version $($existing.Fields['SchemaVersion']); only schema $script:RecoverySchemaVersion is supported."
    }
    foreach ($name in $script:RequiredRecoveryFields) {
        if (-not $existing.Fields.ContainsKey($name)) {
            throw 'The existing recovery backup is incomplete; restore from it is not possible and shell activation is refused.'
        }
    }
    if ($existing.Active) {
        if (-not [string]::Equals($existing.Fields['ShellCommand'], $ShellCommand, [StringComparison]::Ordinal)) {
            throw 'A Tessera shell is already active; restore it first before activating again.'
        }
        # Ownership means the exact REG_SZ command; a foreign value with the
        # same bytes but a different registry type is not ours to keep.
        $isOwnOverride = ($Current.Present -and
            $Current.Kind -eq [Microsoft.Win32.RegistryValueKind]::String -and
            [string]::Equals($Current.Value, $ShellCommand, [StringComparison]::Ordinal))
        if (-not $isOwnOverride) {
            throw 'The active shell configuration was changed outside this deployment; restore it first.'
        }
    } else {
        $expectedPresent = [int]$existing.Fields['OriginalPresent'] -eq 1
        $storedOriginal = @{
            Present = $expectedPresent
            Kind = [int]$existing.Fields['OriginalKind']
            Value = [string]$existing.Fields['OriginalValue']
        }
        if (-not (Test-DeploymentRestoreMatches -Current $Current -Original $storedOriginal)) {
            throw 'The shell configuration no longer matches the stored original; restore it first.'
        }
    }
    return @{
        Present = [int]$existing.Fields['OriginalPresent'] -eq 1
        Kind = [int]$existing.Fields['OriginalKind']
        Value = [string]$existing.Fields['OriginalValue']
    }
}

function Read-DeploymentBackupKey {
    $state = Get-ShellRecoveryState
    return @{ Exists = [bool]$state.Exists; Active = [bool]$state.Active; Fields = $state.Fields }
}

function Write-DeploymentRecoveryFields {
    param(
        [Parameter(Mandatory)][AllowNull()][Microsoft.Win32.RegistryKey]$Key,
        [Parameter(Mandatory)][string]$ShellCommand,
        [Parameter(Mandatory)][string]$InstallDirectory,
        [Parameter(Mandatory)]$Original
    )
    $existing = Read-DeploymentBackupKey
    if ($script:DryRun) {
        # A WhatIf run opens the recovery key read-only; it may be null when
        # no backup exists yet, and a plan must not create it. A blank
        # record (no SchemaVersion yet) is likewise nothing to update, so
        # WhatIf writes nothing in either case. Writing the backup fields
        # is the first real (non-WhatIf) side effect.
        if ($null -eq $Key -or -not $existing.Fields.ContainsKey('SchemaVersion')) {
            Write-Host 'WHATIF: Would create the shell recovery backup key and record the deployment fields.'
        }
        return
    }
    if (-not $existing.Exists) {
        $Key.SetValue('SchemaVersion', $script:RecoverySchemaVersion, [Microsoft.Win32.RegistryValueKind]::DWord)
        $Key.SetValue('Active', 0, [Microsoft.Win32.RegistryValueKind]::DWord)
        $Key.SetValue('OriginalPresent', [int]$Original.Present, [Microsoft.Win32.RegistryValueKind]::DWord)
        $Key.SetValue('OriginalKind', [int]$Original.Kind, [Microsoft.Win32.RegistryValueKind]::DWord)
        $Key.SetValue('OriginalValue', [string]$Original.Value, [Microsoft.Win32.RegistryValueKind]::String)
        $Key.SetValue('ShellCommand', $ShellCommand, [Microsoft.Win32.RegistryValueKind]::String)
        $Key.SetValue('InstallDirectory', $InstallDirectory, [Microsoft.Win32.RegistryValueKind]::String)
        return
    }
    if (-not [string]::Equals($existing.Fields['ShellCommand'], $ShellCommand, [StringComparison]::Ordinal) -or
        -not [string]::Equals($existing.Fields['InstallDirectory'], $InstallDirectory, [StringComparison]::Ordinal)) {
        if ($existing.Active) {
            throw 'The stored recovery backup points to a different deployment; restore it first.'
        }
        # A new deployment on a restored (inactive) backup updates only the
        # deployment identity fields; the original fields stay protected.
        $Key.SetValue('ShellCommand', $ShellCommand, [Microsoft.Win32.RegistryValueKind]::String)
        $Key.SetValue('InstallDirectory', $InstallDirectory, [Microsoft.Win32.RegistryValueKind]::String)
    }
}

function Set-DeploymentActiveState {
    param(
        [Parameter(Mandatory)][AllowNull()][Microsoft.Win32.RegistryKey]$Key,
        [Parameter(Mandatory)][bool]$Active
    )
    if ($script:DryRun) { return }
    $Key.SetValue('Active', [int][bool]$Active, [Microsoft.Win32.RegistryValueKind]::DWord)
    $raw = Get-DeploymentRawValue -Key $Key -Name 'Active'
    if (-not $raw.Present -or $raw.Kind -ne [Microsoft.Win32.RegistryValueKind]::DWord -or
        [int]$Key.GetValue('Active') -ne [int][bool]$Active) {
        throw 'The recovery Active flag did not verify after writing.'
    }
}

# Pure typed comparison of a raw shell value against the recorded original.
# Returns $true only for exact presence, registry kind, and byte-equal data.
# Callers that may accept an alternative (for example the recorded Tessera
# command) check it separately; this helper is the single definition of
# 'the shell configuration equals the stored original'.
function Test-DeploymentRestoreMatches {
    param(
        [Parameter(Mandatory)]$Current,
        [Parameter(Mandatory)]$Original
    )
    $expectedPresent = [bool]$Original.Present
    if ([bool]$Current.Present -ne $expectedPresent) { return $false }
    if (-not $expectedPresent) { return $true }
    $kindCode = $null
    try { $kindCode = [int]$Original.Kind } catch { $kindCode = $null }
    if ($null -eq $kindCode) { throw "The stored original registry type code '$($Original.Kind)' is not an integer." }
    $kind = ConvertFrom-RegistryTypeCode -TypeCode $kindCode
    if ($null -eq $kind) { throw "The stored original registry type code $kindCode is not supported." }
    return ($Current.Kind -eq $kind -and [string]::Equals($Current.Value, $Original.Value, [StringComparison]::Ordinal))
}

function Restore-DeploymentShellValue {
    param(
        [Parameter(Mandatory)][Microsoft.Win32.RegistryKey]$Winlogon,
        [Parameter(Mandatory)]$Original
    )
    # Writes or deletes the recorded original and verifies the exact result
    # (presence, kind, and bytes) by readback before returning. The caller
    # must only publish Active=0 after this helper has confirmed the
    # restore, so the same typed policy governs rollback and recovery.
    if ($Original.Present) {
        $kindCode = $null
        try { $kindCode = [int]$Original.Kind } catch { $kindCode = $null }
        if ($null -eq $kindCode) { throw "The stored original registry type code '$($Original.Kind)' is not an integer." }
        $kind = ConvertFrom-RegistryTypeCode -TypeCode $kindCode
        if ($null -eq $kind) { throw "The stored original registry type code $kindCode is not supported." }
        $Winlogon.SetValue($script:ShellValueName, [string]$Original.Value, $kind)
    } elseif (@($Winlogon.GetValueNames()) -contains $script:ShellValueName) {
        $Winlogon.DeleteValue($script:ShellValueName, $false)
    }
    $restored = Get-DeploymentRawValue -Key $Winlogon -Name $script:ShellValueName
    $verified = Test-DeploymentRestoreMatches -Current $restored -Original $Original
    if (-not $verified) {
        throw "The restored Shell value did not verify after being written; expected present=$($Original.Present) kind=$($Original.Kind)."
    }
}

# Restores the recorded original shell value and clears Active only after
# the exact original value and registry kind are confirmed. Ownership is
# typed (exact REG_SZ command or the exact stored original); a foreign
# same-bytes REG_EXPAND_SZ value or a foreign deletion is refused and the
# backup retained. The Winlogon key itself is never deleted.
function Invoke-ShellRestore {
    [CmdletBinding(SupportsShouldProcess)][OutputType([pscustomobject])]
    param()

    $previousDryRun = $script:DryRun
    $script:DryRun = [bool]$WhatIfPreference
    try {
        $backup = Read-DeploymentBackupKey
        if (-not $backup.Exists) {
            throw 'No Tessera shell recovery backup exists; there is nothing to restore.'
        }
        # Read-DeploymentBackupKey decodes every required field strictly and
        # fail-closed: a missing field, a wrong registry type, an out-of-range
        # counter, or an unsupported schema version throws before any policy
        # decision, so the former manual completeness loop is redundant.
        if (-not $backup.Active) {
            Write-Verbose 'The Tessera shell is not marked active; nothing to restore.'
            return New-DeploymentRestoreResult -Action 'AlreadyRestored'
        }

        $original = @{
            Present = [int]$backup.Fields['OriginalPresent'] -eq 1
            Kind = [int]$backup.Fields['OriginalKind']
            Value = [string]$backup.Fields['OriginalValue']
        }
        $shellCommand = [string]$backup.Fields['ShellCommand']

        $winlogon = Open-DeploymentRegistryKey -Scope User -SubKey $script:DeploymentContext.WinlogonSubKey -Writable
        if ($null -eq $winlogon) {
            throw 'The per-user Winlogon key could not be opened for restore.'
        }
        try {
            $current = Get-DeploymentRawValue -Key $winlogon -Name $script:ShellValueName
            if ($current.Present -and $null -eq $current.Value) {
                throw "The current Shell value has registry type $($current.Kind), which this deployment does not manage; refusing to overwrite it."
            }
            # Typed ownership policy. The current value belongs to this
            # deployment only when it is exactly the recorded command with
            # the recorded REG_SZ kind, or already equals the recorded
            # original with the exact original kind and presence. A mere
            # string match with a different registry type (for example a
            # foreign REG_EXPAND_SZ with the same bytes) is a foreign
            # change and is refused; a missing value that the original
            # recorded as present is likewise a foreign deletion.
            $currentIsOwnCommand = ($current.Present -and
                $current.Kind -eq [Microsoft.Win32.RegistryValueKind]::String -and
                [string]::Equals($current.Value, $shellCommand, [StringComparison]::Ordinal))
            $currentMatchesOriginal = Test-DeploymentRestoreMatches -Current $current -Original $original
            if (-not $currentIsOwnCommand -and -not $currentMatchesOriginal) {
                throw 'The shell value was changed by something other than this deployment; refusing to overwrite it and keeping the backup.'
            }
            if ($currentMatchesOriginal) {
                # The shell already equals the recorded original; only the
                # active flag is cleared.
                $recovery = Open-DeploymentRegistryKey -Scope User -SubKey $script:DeploymentContext.RecoverySubKey -Writable
                try {
                    Set-DeploymentActiveState -Key $recovery -Active $false
                } finally {
                    if ($null -ne $recovery) { $recovery.Dispose() }
                }
                Write-Verbose 'The shell value already equals the recorded original; only the active flag was cleared.'
                return New-DeploymentRestoreResult -Action 'AlreadyOriginal'
            }
            if ($script:DryRun) {
                Write-Host 'WHATIF: Would restore the recorded original shell value'
                return New-DeploymentRestoreResult -Action 'WouldRestore'
            }
            # Ownership held (exact REG_SZ command); write the recorded
            # original. Restore-DeploymentShellValue verifies the exact
            # typed presence, kind, and bytes by readback, so Active is
            # cleared only after that proof.
            Restore-DeploymentShellValue -Winlogon $winlogon -Original $original
            $recovery = Open-DeploymentRegistryKey -Scope User -SubKey $script:DeploymentContext.RecoverySubKey -Writable
            try {
                Set-DeploymentActiveState -Key $recovery -Active $false
            } finally {
                if ($null -ne $recovery) { $recovery.Dispose() }
            }
            Write-Verbose 'The original shell configuration was restored.'
            return New-DeploymentRestoreResult -Action 'Restored'
        } finally {
            if ($null -ne $winlogon) { $winlogon.Dispose() }
        }
    } finally {
        $script:DryRun = $previousDryRun
    }
}

function New-DeploymentRestoreResult {
    param([Parameter(Mandatory)][ValidateSet('Restored', 'AlreadyRestored', 'AlreadyOriginal', 'WouldRestore')][string]$Action)
    return [pscustomobject]@{ Action = $Action }
}

# Mandatory supervisor runtime verification: launches the INSTALLED
# supervisor's own '--verify-runtime' probe (self-verification that the
# unsigned supervisor can start at all, e.g. under Smart App Control),
# waits for exit 0 within a bounded time, then terminates only the process
# it started if still running. It writes no registry data, never touches
# Winlogon or Explorer, and starts no other program. Tests inject a safe
# probe through the module-internal Hooks seam; production never sets it.
function Invoke-SupervisorRuntimeVerification {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$SupervisorPath)

    if ($script:DeploymentContext.Hooks.ContainsKey('SupervisorRuntimeProbe')) {
        & $script:DeploymentContext.Hooks['SupervisorRuntimeProbe'] -SupervisorPath $SupervisorPath
        return
    }

    if (-not (Test-Path -LiteralPath $SupervisorPath -PathType Leaf)) {
        throw "The installed shell supervisor is missing: $SupervisorPath"
    }

    # Bounded wait: 30 s supervisor startup + 30 s UI heartbeat, plus slack.
    $waitMilliseconds = 90000
    $probe = $null
    try {
        $probe = Start-Process -FilePath $SupervisorPath -ArgumentList '--verify-runtime' -PassThru -Wait:$false
        if (-not $probe.WaitForExit($waitMilliseconds)) {
            if (-not $probe.HasExited) {
                $null = $probe.Kill()
                $probe.WaitForExit(5000) | Out-Null
            }
            throw ("The supervisor runtime verification did not finish within 90 seconds; " +
                "the shell activation is refused.")
        }
        if ($probe.ExitCode -ne 0) {
            throw ("The supervisor runtime verification failed with exit code $($probe.ExitCode); " +
                "the shell activation is refused. The supervisor may be blocked by security policy.")
        }
    } finally {
        if ($null -ne $probe -and -not $probe.HasExited) {
            try { $null = $probe.Kill() } catch { }
        }
        if ($null -ne $probe) { $probe.Dispose() }
    }
    Write-Verbose 'The supervisor runtime verification probe completed successfully.'
}

Export-ModuleMember -Function @(
    'Test-TesseraPackage',
    'Test-ShellSupport',
    'Copy-TesseraPackage',
    'Publish-RecoveryRuntime',
    'Invoke-ShellActivation',
    'Invoke-ShellRestore',
    'Invoke-SupervisorRuntimeVerification',
    'Get-ShellRecoveryState',
    'Get-DeploymentRawValue',
    'Get-TesseraRecoveryDirectory',
    'Get-TesseraInstallRoot',
    'Get-DeploymentLockPath',
    'New-DeploymentLock'
)
