# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
#
# Focused deployment tests, runnable on Windows PowerShell 5.1 without
# Pester. They drive the module through its documented public engine seam by
# replacing the module context with temporary user-registry-subtree and
# temporary filesystem fixtures, so no test touches the real Winlogon paths
# or the real LOCALAPPDATA. No processes are launched.
#
# Run from the repository root:  powershell -File scripts\tests\deployment.tests.ps1

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$modulePath = Join-Path $repoRoot 'scripts\Tessera.Deployment.psm1'
$module = Import-Module $modulePath -Force -PassThru

# The deployment context, the private helpers, and $script:RequiredRecoveryFields
# live in the module's session state and are deliberately invisible to this
# script. Every fixture read or write therefore goes through a scriptblock
# bound to the module with & $module { ... }; the helpers below wrap those
# blocks so the test bodies stay readable.

function Get-ModuleContext {
    # A shallow copy: the test keeps fixture-previous values while the live
    # table keeps being mutated by later fixtures.
    & $module { return $script:DeploymentContext.Clone() }
}

function Set-ModuleContext {
    param([Parameter(Mandatory)][hashtable]$Values)
    & $module {
        param($Values)
        foreach ($entry in $Values.GetEnumerator()) {
            $script:DeploymentContext[$entry.Key] = $entry.Value
        }
    } $Values
}

$script:PassCount = 0
$script:Failures = @()

# Minimal placeholder "PE" file for the module's architecture check:
# a DOS header whose e_lfanew field (at 0x3c) points at offset 0x40, where
# an 'PE\0\0' signature and the x86-64 machine code 0x8664 follow.
$script:PlaceholderPe = [byte[]]([byte[]](0x4D, 0x5A) + [byte[]]::new(58) +
    [byte[]](0x40, 0x00, 0x00, 0x00) + [byte[]](0x50, 0x45, 0x00, 0x00) +
    [byte[]](0x64, 0x86))

function Assert-DeploymentEqual {
    param($Actual, $Expected, [string]$Message)
    $same = $false
    try { $same = ($Actual -eq $Expected) } catch { $same = $false }
    if (-not $same) {
        # Fallback comparison for non-scalar values.
        $same = (ConvertTo-Json -InputObject $Actual -Depth 6 -Compress) -eq
            (ConvertTo-Json -InputObject $Expected -Depth 6 -Compress)
    }
    if ($same) {
        $script:PassCount++
    } else {
        $script:Failures += "$Message`n    expected: $(ConvertTo-Json -InputObject $Expected -Depth 6 -Compress)`n    actual:   $(ConvertTo-Json -InputObject $Actual -Depth 6 -Compress)"
    }
}

function Assert-DeploymentTrue {
    param($Condition, [string]$Message)
    if ($Condition) { $script:PassCount++ } else { $script:Failures += $Message }
}

function Assert-DeploymentFalse {
    param($Condition, [string]$Message)
    if ($Condition) { $script:Failures += $Message } else { $script:PassCount++ }
}

function Assert-DeploymentThrows {
    param([scriptblock]$Body, [string]$MessagePattern, [string]$Message)
    $errorText = $null
    try {
        & $Body
    } catch {
        $errorText = $_.Exception.Message
    }
    if ($null -eq $errorText) {
        $script:Failures += "$Message (no error was thrown)"
    } elseif ($errorText -notmatch $MessagePattern) {
        $script:Failures += "$Message`n    expected match: $MessagePattern`n    actual error:   $errorText"
    } else {
        $script:PassCount++
    }
}

function New-DeploymentFixture {
    # Builds a temporary user-registry-subtree and temporary filesystem
    # fixture and points the module's internal deployment context at them
    # through a module-bound scriptblock. Every key the production code
    # touches lives under the fixture root: both hive handles point at
    # HKCU and every machine-scope subkey (OS gate, Smart App Control,
    # machine Winlogon) is redirected under the same fixture prefix.
    $registryRoot = 'HKCU:\TesseraDeploymentTest\' + [Guid]::NewGuid().ToString('N')
    $fileRoot = Join-Path ([IO.Path]::GetTempPath()) ('tessera-deploy-test-' + [Guid]::NewGuid().ToString('N'))
    $packageRoot = Join-Path $fileRoot 'package'
    New-Item -ItemType Directory -Path $packageRoot | Out-Null

    # Minimal placeholder "PE" files for the module's architecture check.
    foreach ($name in 'Tessera.exe', 'tessera-cli.exe', 'tessera-shell.exe') {
        [IO.File]::WriteAllBytes((Join-Path $packageRoot $name), $script:PlaceholderPe)
    }
    @("Tessera 0.1.0-alpha.2", "Source commit: 0123456789abcdef0123456789abcdef01234567", "Target: x86_64-pc-windows-msvc") |
        Set-Content (Join-Path $packageRoot 'BUILD-INFO.txt') -Encoding utf8

    # Copy the real deployment scripts and doc stubs so entrypoint-level
    # tests can validate a complete package without depending on a release.
    foreach ($name in 'Install-Tessera.ps1', 'Restore-Tessera.ps1', 'Tessera.Deployment.psm1') {
        Copy-Item -LiteralPath (Join-Path $repoRoot "scripts\$name") -Destination (Join-Path $packageRoot $name)
    }
    foreach ($name in 'LICENSE.txt', 'START-HERE.txt', 'THIRD-PARTY-NOTICES.txt') {
        Set-Content -LiteralPath (Join-Path $packageRoot $name) -Value 'placeholder' -Encoding ascii
    }

    # Both hive handles point at HKCU: the module-internal fixture seam is
    # a documented test-only surface, and every machine-scope subkey the
    # engine reads is redirected below so no real HKLM path is touched.
    $userHive = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
        [Microsoft.Win32.RegistryHive]::CurrentUser,
        [Microsoft.Win32.RegistryView]::Registry64)
    $prefix = $registryRoot -replace '^HKCU:\\', ''
    $winlogonPath = "$prefix\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon"
    $recoveryPath = "$prefix\SOFTWARE\Tessera\ShellRecovery"
    $policyPath = "$prefix\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
    $sacPolicyPath = "$prefix\SYSTEM\CurrentControlSet\Control\CI\Policy"
    $osPath = "$prefix\SOFTWARE\Microsoft\Windows NT\CurrentVersion"

    # A supported-host baseline inside the fixture: the OS and SAC gates read
    # machine-scope keys, which the fixture redirects, so a clean fixture
    # passes Test-ShellSupport regardless of the real host. Individual tests
    # overwrite these values to exercise the refusals.
    $osBaseline = $userHive.CreateSubKey($osPath)
    $osBaseline.SetValue('CurrentBuildNumber', '22000', [Microsoft.Win32.RegistryValueKind]::String)
    $osBaseline.SetValue('InstallationType', 'Client', [Microsoft.Win32.RegistryValueKind]::String)
    $osBaseline.Dispose()
    $sacBaseline = $userHive.CreateSubKey($sacPolicyPath)
    # VerifiedAndReputablePolicyState 0 = Off (documented; probe still mandatory).
    $sacBaseline.SetValue('VerifiedAndReputablePolicyState', 0, [Microsoft.Win32.RegistryValueKind]::DWord)
    $sacBaseline.Dispose()

    $previous = Get-ModuleContext
    Set-ModuleContext @{
        UserHive = $userHive
        MachineHive = $userHive
        WinlogonSubKey = $winlogonPath
        RecoverySubKey = $recoveryPath
        PolicySystemSubKey = $policyPath
        SmartAppControlPolicySubKey = $sacPolicyPath
        OsSubKey = $osPath
        LocalAppDataRoot = $fileRoot
        LockRetryMilliseconds = 10
        DomainProbe = { $false }.GetNewClosure()
    }

    return @{
        UserHive = $userHive
        WinlogonKey = $userHive.CreateSubKey($winlogonPath)
        RecoveryKey = $userHive.CreateSubKey($recoveryPath)
        PolicyKey = $userHive.CreateSubKey($policyPath)
        SmartAppControlPolicyKey = $userHive.CreateSubKey($sacPolicyPath)
        OsKey = $userHive.CreateSubKey($osPath)
        PackagePath = $packageRoot
        FileRoot = $fileRoot
        RegistryRoot = $registryRoot
        Previous = $previous
    }
}

function Remove-DeploymentFixture {
    param([Parameter(Mandatory)]$Fixture)
    foreach ($hive in @($Fixture.WinlogonKey, $Fixture.RecoveryKey, $Fixture.PolicyKey,
        $Fixture.SmartAppControlPolicyKey, $Fixture.OsKey, $Fixture.UserHive)) {
        $hive.Dispose()
    }
    Remove-Item -LiteralPath $Fixture.RegistryRoot -Recurse -Force
    Remove-Item -LiteralPath $Fixture.FileRoot -Recurse -Force
    foreach ($property in 'UserHive', 'MachineHive', 'WinlogonSubKey', 'RecoverySubKey',
        'PolicySystemSubKey', 'SmartAppControlPolicySubKey', 'OsSubKey',
        'LocalAppDataRoot', 'LockRetryMilliseconds', 'DomainProbe') {
        Set-ModuleContext @{ $property = $Fixture.Previous[$property] }
    }
    Set-ModuleContext @{ Hooks = @{} }
}

# Reads through the same module seam the production code uses, so the
# assertions exercise the contract rather than a parallel implementation.
function Get-FixtureShellValue {
    param([Parameter(Mandatory)]$Fixture)
    $raw = Get-DeploymentRawValue -Key $Fixture.WinlogonKey -Name 'Shell'
    return $raw
}

function Get-FixtureRecoveryFields {
    param([Parameter(Mandatory)]$Fixture)
    $fields = @{}
    foreach ($name in (Get-ModuleRequiredRecoveryFields)) {
        $raw = Get-DeploymentRawValue -Key $Fixture.RecoveryKey -Name $name
        if ($raw.Present -and $raw.Kind -eq [Microsoft.Win32.RegistryValueKind]::DWord) {
            $fields[$name] = [int]$Fixture.RecoveryKey.GetValue($name)
        } elseif ($raw.Present) {
            $fields[$name] = [string]$raw.Value
        }
    }
    return $fields
}

function Set-FixtureShellValue {
    param(
        [Parameter(Mandatory)]$Fixture,
        [AllowEmptyString()][string]$Value,
        [Microsoft.Win32.RegistryValueKind]$Kind = [Microsoft.Win32.RegistryValueKind]::String
    )
    if ($null -eq $Value) {
        $Fixture.WinlogonKey.DeleteValue('Shell', $false)
    } else {
        $Fixture.WinlogonKey.SetValue('Shell', $Value, $Kind)
    }
}

function Set-DeploymentHook {
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][scriptblock]$Body)
    # NOTE: arguments to a module-bound scriptblock must be space-separated;
    # a comma would make PowerShell pass a single array to the first param.
    & $module {
        param($hookName, $hookBody)
        $script:DeploymentContext.Hooks[$hookName] = $hookBody
    } $Name $Body
}

function Get-ModuleRequiredRecoveryFields {
    & $module { return $script:RequiredRecoveryFields }
}

# Restores one backup field to its previously captured typed content so the
# strict-decode test cases stay independent.
function Restore-FixtureRecoveryField {
    param(
        [Parameter(Mandatory)]$Fixture,
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)]$Before
    )
    if (-not $Before.ContainsKey($Name)) {
        $Fixture.RecoveryKey.DeleteValue($Name, $false)
        return
    }
    $value = $Before[$Name]
    $isCounter = $Name -in 'SchemaVersion', 'Active', 'OriginalPresent', 'OriginalKind'
    $kind = if ($isCounter) {
        [Microsoft.Win32.RegistryValueKind]::DWord
    } else {
        [Microsoft.Win32.RegistryValueKind]::String
    }
    $Fixture.RecoveryKey.SetValue($Name, $value, $kind)
}

# ---------------------------------------------------------------- test cases

function Test-PackageValidation {
    $fixture = New-DeploymentFixture
    try {
        $info = Test-TesseraPackage -PackagePath $fixture.PackagePath
        Assert-DeploymentEqual $info.Version '0.1.0-alpha.2' 'Package validation reads the version.'
        Assert-DeploymentEqual $info.Commit '0123456789abcdef0123456789abcdef01234567' 'Package validation reads the commit.'
        Assert-DeploymentTrue ((Split-Path -Leaf $info.SupervisorPath) -eq 'tessera-shell.exe') 'Supervisor path points at tessera-shell.exe.'

        Remove-Item -LiteralPath (Join-Path $fixture.PackagePath 'tessera-shell.exe')
        Assert-DeploymentThrows { Test-TesseraPackage -PackagePath $fixture.PackagePath } 'incomplete' 'Incomplete package is refused.'

        # Idempotent rerun of identical content and refusal of differing
        # content for the same immutable version directory.
        foreach ($name in 'Tessera.exe', 'tessera-cli.exe', 'tessera-shell.exe') {
            [IO.File]::WriteAllBytes((Join-Path $fixture.PackagePath $name), $script:PlaceholderPe)
        }
        $first = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        $second = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        Assert-DeploymentEqual $first $second 'Identical rerun of the same version is idempotent.'

        # A differing package with the same version is refused; the
        # installed copy stays intact.
        [IO.File]::WriteAllText((Join-Path $fixture.PackagePath 'BUILD-INFO.txt'), "Tessera 0.1.0-alpha.2`r`nSource commit: fedcba9876543210fedcba9876543210fedcba98`r`n")
        Assert-DeploymentThrows {
            Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        } 'immutable' 'Different content for an installed version is refused.'
        Assert-DeploymentTrue (Test-Path -LiteralPath $first) 'The installed directory survives the refused overwrite.'

        # WhatIf over an already-installed version must not stage a real copy.
        $stagingBefore = @(Get-ChildItem -LiteralPath (Get-TesseraInstallRoot) -Filter '.staging-*' -Force).Count
        $whatifResult = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2' -WhatIf
        Assert-DeploymentEqual $whatifResult $first 'WhatIf over an installed version reports the installed path.'
        Assert-DeploymentEqual @(Get-ChildItem -LiteralPath (Get-TesseraInstallRoot) -Filter '.staging-*' -Force).Count $stagingBefore 'WhatIf over an installed version stages nothing.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-ActivationAndRestoreWithoutShellValue {
    $fixture = New-DeploymentFixture
    try {
        $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        Publish-RecoveryRuntime -SourceDirectory (Join-Path $repoRoot 'scripts')
        Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed

        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.SchemaVersion 1 'Fresh backup schema version.'
        Assert-DeploymentEqual $fields.Active 1 'Fresh backup is active.'
        Assert-DeploymentEqual $fields.OriginalPresent 0 'Fresh backup records the missing original.'
        Assert-DeploymentEqual $fields.OriginalKind 1 'Fresh backup defaults the original kind to REG_SZ.'
        Assert-DeploymentEqual $fields.OriginalValue '' 'Fresh backup stores an empty original value.'
        Assert-DeploymentTrue ($fields.ShellCommand -eq ('"' + $installed + '\tessera-shell.exe"')) 'Backup stores the exact quoted supervisor command.'
        Assert-DeploymentEqual $fields.InstallDirectory $installed 'Backup stores the absolute install directory.'

        $shell = Get-FixtureShellValue -Fixture $fixture
        Assert-DeploymentEqual $shell.Present $true 'The override was written.'
        Assert-DeploymentEqual $shell.Kind ([Microsoft.Win32.RegistryValueKind]::String) 'The override is REG_SZ.'
        Assert-DeploymentEqual $shell.Value ('"' + $installed + '\tessera-shell.exe"') 'The override command is exact.'

        $result = Invoke-ShellRestore
        Assert-DeploymentEqual $result.Action 'Restored' 'Restore reports success.'
        $shell = Get-FixtureShellValue -Fixture $fixture
        Assert-DeploymentEqual $shell.Present $false 'The absent original is restored by deleting the Shell value.'
        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.Active 0 'Active is cleared only after verified restore.'
        Assert-DeploymentEqual $fields.OriginalPresent 0 'The backup is retained as evidence.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-ActivationAndRestoreWithStringAndExpandString {
    foreach ($kind in [Microsoft.Win32.RegistryValueKind]::String, [Microsoft.Win32.RegistryValueKind]::ExpandString) {
        $fixture = New-DeploymentFixture
        try {
            if ($kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) {
                Set-FixtureShellValue -Fixture $fixture -Value '%SystemRoot%\explorer.exe' -Kind $kind
            } else {
                Set-FixtureShellValue -Fixture $fixture -Value 'explorer.exe' -Kind $kind
            }
            $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
            Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed

            $fields = Get-FixtureRecoveryFields -Fixture $fixture
            $expectedKind = if ($kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) { 2 } else { 1 }
            Assert-DeploymentEqual $fields.OriginalPresent 1 "$kind original present flag."
            Assert-DeploymentEqual $fields.OriginalKind $expectedKind "$kind original kind code."
            if ($kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) {
                Assert-DeploymentEqual $fields.OriginalValue '%SystemRoot%\explorer.exe' 'ExpandString original is stored unexpanded.'
            } else {
                Assert-DeploymentEqual $fields.OriginalValue 'explorer.exe' 'String original is stored verbatim.'
            }

            $result = Invoke-ShellRestore
            Assert-DeploymentEqual $result.Action 'Restored' "$kind restore reports success."
            $shell = Get-FixtureShellValue -Fixture $fixture
            Assert-DeploymentEqual $shell.Kind $kind "$kind original registry kind is restored."
            if ($kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) {
                Assert-DeploymentEqual $shell.Value '%SystemRoot%\explorer.exe' 'ExpandString data is restored unexpanded.'
            } else {
                Assert-DeploymentEqual $shell.Value 'explorer.exe' 'String data is restored verbatim.'
            }
        } finally {
            Remove-DeploymentFixture $fixture
        }
    }
}

function Test-IdempotentRerunAndForeignShellRefusal {
    $fixture = New-DeploymentFixture
    try {
        $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed

        # Idempotent rerun with the same command succeeds and does not
        # replace the recorded original.
        Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.OriginalPresent 0 'Rerun keeps the original absence backup.'
        Assert-DeploymentEqual $fields.Active 1 'Rerun keeps the deployment active.'

        # A foreign shell change after activation is refused.
        Set-FixtureShellValue -Fixture $fixture -Value 'C:\something-else.exe'
        Assert-DeploymentThrows {
            Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
        } 'changed outside' 'A changed shell value is not clobbered by reactivation.'
        $shell = Get-FixtureShellValue -Fixture $fixture
        Assert-DeploymentEqual $shell.Value 'C:\something-else.exe' 'The foreign value is retained.'

        # Restore refuses too while the current value is foreign.
        Assert-DeploymentThrows {
            Invoke-ShellRestore
        } 'changed by something other' 'Restore refuses a foreign value and keeps the backup.'
        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.Active 1 'The backup remains active for later recovery.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-TypedOwnershipOfForeignExpandString {
    # A foreign REG_EXPAND_SZ value with byte-identical data is NOT owned by
    # this deployment: reactivation, restore, and preflight all refuse, and
    # the Active flag is retained. The recorded original REG_SZ with the same
    # bytes stays untouched.
    foreach ($pair in @(
        @{ OriginalKind = [Microsoft.Win32.RegistryValueKind]::String; OriginalValue = 'explorer.exe' },
        @{ OriginalKind = [Microsoft.Win32.RegistryValueKind]::ExpandString; OriginalValue = '%SystemRoot%\explorer.exe' }
    )) {
        $fixture = New-DeploymentFixture
        try {
            Set-FixtureShellValue -Fixture $fixture -Value $pair.OriginalValue -Kind $pair.OriginalKind
            $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
            Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
            $fields = Get-FixtureRecoveryFields -Fixture $fixture
            $expectedKind = if ($pair.OriginalKind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) { 2 } else { 1 }
            Assert-DeploymentEqual $fields.OriginalKind $expectedKind 'The original kind is recorded before the foreign rewrite.'

            # A foreign REG_EXPAND_SZ carrying the exact override bytes.
            Set-FixtureShellValue -Fixture $fixture -Value ('"' + $installed + '\tessera-shell.exe"') -Kind ([Microsoft.Win32.RegistryValueKind]::ExpandString)
            Assert-DeploymentThrows {
                Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
            } 'changed outside' 'Reactivation refuses the same-bytes REG_EXPAND_SZ value.'

            Assert-DeploymentThrows {
                Invoke-ShellRestore
            } 'changed by something other' 'Restore refuses the same-bytes REG_EXPAND_SZ value.'

            $shell = Get-FixtureShellValue -Fixture $fixture
            Assert-DeploymentEqual $shell.Kind ([Microsoft.Win32.RegistryValueKind]::ExpandString) 'The foreign kind is retained.'
            $fields = Get-FixtureRecoveryFields -Fixture $fixture
            Assert-DeploymentEqual $fields.Active 1 'The backup stays active after the foreign typed rewrite.'
            Assert-DeploymentEqual $fields.OriginalKind $expectedKind 'The original kind is retained after the foreign typed rewrite.'
            $changedCase = ('"' + $installed + '\tessera-shell.exe"').ToUpperInvariant()
            Set-FixtureShellValue -Fixture $fixture -Value $changedCase
            Assert-DeploymentThrows { Invoke-ShellRestore } 'changed by something other' 'Raw command ownership is ordinal, even on a case-insensitive filesystem.'
            Assert-DeploymentEqual (Get-FixtureShellValue -Fixture $fixture).Value $changedCase 'A case-only foreign rewrite is retained.'
        } finally {
            Remove-DeploymentFixture $fixture
        }
    }
}

function Test-RestoreRefusesForeignDeletion {
    # A value the backup records as present must not be treated as restored
    # when a third party deleted it: the foreign deletion is refused and the
    # backup stays active.
    $fixture = New-DeploymentFixture
    try {
        Set-FixtureShellValue -Fixture $fixture -Value 'explorer.exe'
        $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.OriginalPresent 1 'The original presence is recorded before the deletion.'

        $fixture.WinlogonKey.DeleteValue('Shell', $false)
        Assert-DeploymentThrows {
            Invoke-ShellRestore
        } 'changed by something other' 'Restore refuses a foreign deletion of a recorded-present original.'
        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.Active 1 'The backup stays active after a foreign deletion.'
        Assert-DeploymentFalse (Get-FixtureShellValue -Fixture $fixture).Present 'The deletion is not silently accepted as a restore.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-StrictRecoveryStateDecoding {
    # The recovery decode is fail-closed: malformed field types, Active
    # states other than 0/1, unsupported original kinds, missing fields, and
    # unsupported schema versions are all refused; a valid backup still
    # decodes with exact values and the Active flag.
    $fixture = New-DeploymentFixture
    try {
        Assert-DeploymentFalse (Get-ShellRecoveryState).Exists 'A truly empty recovery key is not a backup record.'
        $fixture.RecoveryKey.SetValue('Unrelated', 'foreign', [Microsoft.Win32.RegistryValueKind]::String)
        Assert-DeploymentThrows { Get-ShellRecoveryState } 'recovery' 'A nonempty unknown record is not treated as a fresh backup.'
        Assert-DeploymentThrows {
            Invoke-ShellActivation -InstallDirectory $fixture.PackagePath -PreflightPassed
        } 'recovery' 'Activation refuses a nonempty unknown record before replacing it.'
        Assert-DeploymentEqual $fixture.RecoveryKey.GetValue('Unrelated') 'foreign' 'The unknown record is preserved.'
        $fixture.RecoveryKey.DeleteValue('Unrelated', $false)
        $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
        $state = Get-ShellRecoveryState
        Assert-DeploymentTrue $state.Exists 'An activated backup exists.'
        Assert-DeploymentTrue $state.Active 'An activated backup is active.'
        Assert-DeploymentEqual $state.Fields.SchemaVersion 1 'The schema version decodes exactly.'

        foreach ($case in @(
            @{ Name = 'Active'; Write = { param($k) $k.SetValue('Active', 2, [Microsoft.Win32.RegistryValueKind]::DWord) } },
            @{ Name = 'Active'; Write = { param($k) $k.SetValue('Active', 7, [Microsoft.Win32.RegistryValueKind]::DWord) } },
            @{ Name = 'Active'; Write = { param($k) $k.SetValue('Active', 'one', [Microsoft.Win32.RegistryValueKind]::String) } },
            @{ Name = 'OriginalPresent'; Write = { param($k) $k.SetValue('OriginalPresent', 5, [Microsoft.Win32.RegistryValueKind]::DWord) } },
            @{ Name = 'OriginalKind'; Write = { param($k) $k.SetValue('OriginalKind', 9, [Microsoft.Win32.RegistryValueKind]::DWord) } },
            @{ Name = 'OriginalKind'; Write = { param($k) $k.SetValue('OriginalKind', 'expand', [Microsoft.Win32.RegistryValueKind]::String) } },
            @{ Name = 'ShellCommand'; Write = { param($k) $k.SetValue('ShellCommand', 12345, [Microsoft.Win32.RegistryValueKind]::DWord) } },
            @{ Name = 'ShellCommand'; Write = { param($k) $k.DeleteValue('ShellCommand', $false) } },
            @{ Name = 'SchemaVersion'; Write = { param($k) $k.SetValue('SchemaVersion', 2, [Microsoft.Win32.RegistryValueKind]::DWord) } }
        )) {
            $before = Get-FixtureRecoveryFields -Fixture $fixture
            & $case.Write $fixture.RecoveryKey
            Assert-DeploymentThrows {
                Get-ShellRecoveryState
            } 'recovery' "A malformed $($case.Name) field is refused by the strict decode."
            Assert-DeploymentThrows {
                Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
            } '.' "Reactivation refuses a malformed $($case.Name) field."
            Assert-DeploymentThrows {
                Invoke-ShellRestore
            } '.' "Restore refuses a malformed $($case.Name) field."
            # Restore the valid field content for the next case.
            Restore-FixtureRecoveryField -Fixture $fixture -Name $case.Name -Before $before
        }
        # A foreign Active=0 record is NOT silently re-activated by a rerun.
        $fixture.RecoveryKey.SetValue('Active', 0, [Microsoft.Win32.RegistryValueKind]::DWord)
        Set-FixtureShellValue -Fixture $fixture -Value 'explorer.exe'
        Assert-DeploymentThrows {
            Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
        } 'no longer matches the stored original' 'An inactive backup still enforces the stored original match.'
        Assert-DeploymentEqual (Get-FixtureRecoveryFields -Fixture $fixture).Active 0 'A refused rerun does not flip the foreign Active flag.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-RollbackOnOverrideFailure {
    $fixture = New-DeploymentFixture
    try {
        Set-FixtureShellValue -Fixture $fixture -Value 'explorer.exe'
        $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'
        Set-DeploymentHook -Name 'BeforeShellOverride' -Body { throw 'injected pre-override fault' }
        Assert-DeploymentThrows {
            Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed
        } 'Setting the Shell value failed: injected pre-override fault' 'The injected failure surfaces with its rollback context.'
        $shell = Get-FixtureShellValue -Fixture $fixture
        Assert-DeploymentEqual $shell.Value 'explorer.exe' 'The original value survives the failed override.'
        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.Active 0 'Active is rolled back after a failed override.'
        Assert-DeploymentEqual $fields.OriginalValue 'explorer.exe' 'The original backup survives the failed override.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-UnsupportedActivationHasNoSideEffects {
    # A foreign per-user shell is refused by the caller's preflight
    # (Test-ShellSupport) before Invoke-ShellActivation is ever called, so
    # this drives the documented Install-Tessera.ps1 entrypoint with a
    # module-bound probe hook instead of pre-approving the engine.
    $fixture = New-DeploymentFixture
    try {
        Set-FixtureShellValue -Fixture $fixture -Value 'C:\third-party\explorer.exe'
        Set-DeploymentHook -Name 'SupervisorRuntimeProbe' -Body {
            param([Parameter(Mandatory)][string]$SupervisorPath)
            throw "The supervisor probe must never run for a refused activation: $SupervisorPath"
        }

        $support = Test-ShellSupport
        Assert-DeploymentFalse $support.Supported 'A foreign per-user shell is unsupported.'
        Assert-DeploymentThrows {
            & (Join-Path $repoRoot 'scripts\Install-Tessera.ps1') -PackagePath $fixture.PackagePath -EnableShell
        } 'Shell activation was refused' 'The entrypoint refuses the unsupported activation.'
        Assert-DeploymentEqual (Get-FixtureShellValue -Fixture $fixture).Value 'C:\third-party\explorer.exe' 'A foreign executable named explorer.exe is untouched by refusal.'
        Assert-DeploymentFalse (Test-Path -LiteralPath (Get-TesseraInstallRoot)) 'Nothing is installed when the shell activation is refused.'
        Assert-DeploymentFalse (Test-Path -LiteralPath (Get-DeploymentLockPath)) 'The lock file is released after the refused run.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-PolicyShellIsRefused {
    # A policy-managed Shell value is refused by the caller's preflight, so
    # this drives the documented Install-Tessera.ps1 entrypoint too.
    $fixture = New-DeploymentFixture
    try {
        $fixture.PolicyKey.SetValue('Shell', 'C:\policy-shell.exe', [Microsoft.Win32.RegistryValueKind]::String)
        Set-DeploymentHook -Name 'SupervisorRuntimeProbe' -Body {
            param([Parameter(Mandatory)][string]$SupervisorPath)
            throw "The supervisor probe must never run for a refused activation: $SupervisorPath"
        }

        $support = Test-ShellSupport
        Assert-DeploymentFalse $support.Supported 'A policy-managed Shell value is unsupported.'
        Assert-DeploymentTrue (@($support.Problems | Where-Object { $_ -match 'Policies' }).Count -gt 0) 'The policy problem names the Policies key.'
        Assert-DeploymentThrows {
            & (Join-Path $repoRoot 'scripts\Install-Tessera.ps1') -PackagePath $fixture.PackagePath -EnableShell
        } 'Shell activation was refused' 'The entrypoint is refused while a policy shell is configured.'
        Assert-DeploymentTrue ($null -eq (Get-FixtureShellValue -Fixture $fixture).Kind) 'The Winlogon Shell value is not created by the refusal.'
        Assert-DeploymentFalse (Test-Path -LiteralPath (Get-DeploymentLockPath)) 'The lock file is released after the refused run.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-WhatIfChangesNothing {
    $fixture = New-DeploymentFixture
    try {
        Set-FixtureShellValue -Fixture $fixture -Value 'explorer.exe'
        $before = Get-FixtureRecoveryFields -Fixture $fixture
        Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2' -WhatIf
        Invoke-ShellActivation -InstallDirectory $fixture.PackagePath -WhatIf -PreflightPassed
        # The fixture recovery key exists but has no backup fields yet; an
        # empty record is a missing record, so a WhatIf restore must refuse
        # it without mutating anything.
        Assert-DeploymentThrows {
            Invoke-ShellRestore -WhatIf
        } 'No Tessera shell recovery backup exists' 'Restore-WhatIf with an empty recovery key mutates nothing.'
        $shell = Get-FixtureShellValue -Fixture $fixture
        Assert-DeploymentEqual $shell.Value 'explorer.exe' 'WhatIf does not change the shell value.'
        Assert-DeploymentEqual (Get-FixtureRecoveryFields -Fixture $fixture) $before 'WhatIf leaves no backup fields.'
        Assert-DeploymentFalse (Test-Path -LiteralPath (Join-Path (Get-TesseraInstallRoot) '0.1.0-alpha.2')) 'WhatIf does not copy the package.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-SpaceAndUnicodeInstallDirectory {
    # Space and Unicode characters must survive the full activate/restore
    # cycle verbatim in the quoted command and install-directory fields.
    $fixture = New-DeploymentFixture
    try {
        # PowerShell 5.1 reads BOM-less source as ANSI; build real Unicode
        # characters from ASCII code points without weakening this fixture.
        $unicodeName = 'Tessera ' + [char]0x00E9 + ' ' + [char]0x2014 + ' ' + [char]0x043F + [char]0x044F
        $unicodeRoot = Join-Path $fixture.FileRoot $unicodeName
        $null = [IO.Directory]::CreateDirectory($unicodeRoot)
        foreach ($file in [IO.Directory]::EnumerateFiles($fixture.PackagePath)) {
            [IO.File]::Copy($file, (Join-Path $unicodeRoot ([IO.Path]::GetFileName($file))))
        }
        $installed = Copy-TesseraPackage -PackagePath $unicodeRoot -Version '0.1.0-alpha.2'
        Invoke-ShellActivation -InstallDirectory $installed -PreflightPassed

        $fields = Get-FixtureRecoveryFields -Fixture $fixture
        Assert-DeploymentEqual $fields.ShellCommand ('"' + $installed + '\tessera-shell.exe"') 'Quoted command preserves spaces and Unicode.'
        Assert-DeploymentEqual $fields.InstallDirectory $installed 'Install directory preserves spaces and Unicode.'

        $result = Invoke-ShellRestore
        Assert-DeploymentEqual $result.Action 'Restored' 'Restore works for a space/Unicode install path.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-DeploymentLockContention {
    # Two exclusive FileStream opens on the same lock path must not both
    # succeed; the engine surfaces a lock-held error after its retries.
    $fixture = New-DeploymentFixture
    try {
        $first = New-DeploymentLock
        try {
            Assert-DeploymentThrows {
                New-DeploymentLock
            } 'deployment lock is held' 'A second exclusive lock open is refused.'
        } finally {
            $first.Dispose()
        }
        # The lock is acquirable again after release.
        $second = New-DeploymentLock
        $second.Dispose()
        Assert-DeploymentTrue $true 'The lock is re-acquirable after release.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-EntrypointWhatIfChangesNothing {
    # The public entrypoints are driven with -WhatIf: no lock file, no
    # package copy, no recovery directory, no registry writes, and no
    # process launch. Install with -EnableShell also skips the interactive
    # confirmation and the supervisor probe because WhatIf returns before
    # either is reached.
    $fixture = New-DeploymentFixture
    try {
        Set-FixtureShellValue -Fixture $fixture -Value 'explorer.exe'
        $before = Get-FixtureRecoveryFields -Fixture $fixture
        Set-DeploymentHook -Name 'SupervisorRuntimeProbe' -Body {
            param([Parameter(Mandatory)][string]$SupervisorPath)
            throw 'The supervisor probe must never be reached under -WhatIf.'
        }

        & (Join-Path $repoRoot 'scripts\Install-Tessera.ps1') -PackagePath $fixture.PackagePath -WhatIf
        & (Join-Path $repoRoot 'scripts\Install-Tessera.ps1') -PackagePath $fixture.PackagePath -EnableShell -WhatIf
        & (Join-Path $repoRoot 'scripts\Restore-Tessera.ps1') -WhatIf

        Assert-DeploymentEqual (Get-FixtureShellValue -Fixture $fixture).Value 'explorer.exe' 'Entrypoint WhatIf does not change the shell value.'
        Assert-DeploymentEqual (Get-FixtureRecoveryFields -Fixture $fixture) $before 'Entrypoint WhatIf leaves no backup fields.'
        Assert-DeploymentFalse (Test-Path -LiteralPath (Get-TesseraInstallRoot)) 'Entrypoint WhatIf does not create the install root.'
        Assert-DeploymentFalse (Test-Path -LiteralPath (Get-DeploymentLockPath)) 'Entrypoint WhatIf does not create the deployment lock file.'
        Assert-DeploymentFalse (Test-Path -LiteralPath (Get-TesseraRecoveryDirectory)) 'Entrypoint WhatIf does not create the recovery directory.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-MalformedBuildInfoIsRefused {
    $fixture = New-DeploymentFixture
    try {
        $package = $fixture.PackagePath
        $buildInfoPath = Join-Path $package 'BUILD-INFO.txt'

        # Old speculative key-style format is not accepted.
        [IO.File]::WriteAllText($buildInfoPath, "Version: 0.1.0-alpha.2`r`nCommit: 0123456789abcdef0123456789abcdef01234567`r`n")
        Assert-DeploymentThrows {
            Test-TesseraPackage -PackagePath $package
        } 'numbered alpha version' 'Key-style BUILD-INFO is refused.'

        # Short or malformed source commit is not accepted.
        [IO.File]::WriteAllText($buildInfoPath, "Tessera 0.1.0-alpha.2`r`nSource commit: shorthash`r`n")
        Assert-DeploymentThrows {
            Test-TesseraPackage -PackagePath $package
        } 'Source commit' 'A non-40-hex source commit is refused.'

        # Non-alpha or traversal-looking versions are not accepted.
        [IO.File]::WriteAllText($buildInfoPath, "Tessera ..\..\..\evil`r`nSource commit: 0123456789abcdef0123456789abcdef01234567`r`n")
        Assert-DeploymentThrows {
            Test-TesseraPackage -PackagePath $package
        } 'numbered alpha version' 'A traversal-style version line is refused.'
        [IO.File]::WriteAllText($buildInfoPath, "Tessera 1.0.0`r`nSource commit: 0123456789abcdef0123456789abcdef01234567`r`n")
        Assert-DeploymentThrows {
            Test-TesseraPackage -PackagePath $package
        } 'numbered alpha version' 'A non-alpha version line is refused.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-PreflightGateAndProbeHook {
    # The engine refuses activation without the caller-provided preflight
    # token, and the module-internal probe hook replaces any real process
    # launch (tests never start OS apps).
    $fixture = New-DeploymentFixture
    try {
        $installed = Copy-TesseraPackage -PackagePath $fixture.PackagePath -Version '0.1.0-alpha.2'

        Assert-DeploymentThrows {
            Invoke-ShellActivation -InstallDirectory $installed
        } 'requires a completed preflight' 'Activation without the preflight token is refused.'
        Assert-DeploymentTrue ($null -eq (Get-FixtureShellValue -Fixture $fixture).Kind) 'The refused activation wrote no shell value.'

        Set-DeploymentHook -Name 'SupervisorRuntimeProbe' -Body {
            param([Parameter(Mandatory)][string]$SupervisorPath)
            if (-not (Test-Path -LiteralPath $SupervisorPath -PathType Leaf)) {
                throw "Probe hook received a missing supervisor: $SupervisorPath"
            }
            Write-Verbose 'Probe hook invoked; no process launched.'
        }
        Invoke-SupervisorRuntimeVerification -SupervisorPath (Join-Path $installed 'tessera-shell.exe')
        Assert-DeploymentTrue $true 'The supervisor runtime probe hook is exercised without launching anything.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

function Test-SmartAppControlStateMapping {
    # Documented VerifiedAndReputablePolicyState mapping, verified against
    # Microsoft's primary documentation: 0 = Off, 1 = On (Enforcement),
    # 2 = Evaluation. Enforcement and unverifiable states refuse; Off and
    # Evaluation pass but keep the supervisor probe mandatory.
    $fixture = New-DeploymentFixture
    try {
        $cases = @(
            @{ State = 0; ExpectRefusal = $false },
            @{ State = 1; ExpectRefusal = $true },
            @{ State = 2; ExpectRefusal = $false },
            @{ State = 3; ExpectRefusal = $true },
            @{ State = 99; ExpectRefusal = $true }
        )
        foreach ($case in $cases) {
            $fixture.SmartAppControlPolicyKey.SetValue('VerifiedAndReputablePolicyState', $case.State, [Microsoft.Win32.RegistryValueKind]::DWord)
            $support = Test-ShellSupport
            $sacProblem = @($support.Problems | Where-Object { $_ -match 'Smart App Control' })
            if ($case.ExpectRefusal) {
                Assert-DeploymentTrue ($sacProblem.Count -gt 0) "SAC state $($case.State) produces a refusal problem."
                Assert-DeploymentFalse $support.Supported "SAC state $($case.State) is refused."
            } else {
                Assert-DeploymentTrue ($sacProblem.Count -eq 0) "SAC state $($case.State) does not refuse."
                Assert-DeploymentTrue $support.Supported "The supported client fixture accepts verified SAC state $($case.State)."
            }
        }

        # A non-DWORD or corrupted value cannot be verified and refuses.
        $fixture.SmartAppControlPolicyKey.SetValue('VerifiedAndReputablePolicyState', '0', [Microsoft.Win32.RegistryValueKind]::String)
        Assert-DeploymentFalse (Test-ShellSupport).Supported 'A string pretending to be SAC Off is not a verified DWORD.'
        $fixture.SmartAppControlPolicyKey.DeleteValue('VerifiedAndReputablePolicyState', $false)
        $support = Test-ShellSupport
        Assert-DeploymentFalse $support.Supported 'An unreadable SAC state is refused conservatively.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

# Exercise the real entrypoint through its probe, then stop before prompting.
# A hung GUI or user prompt must never monopolize the native recovery lock.
function Test-InstallerProbeDoesNotHoldRecoveryLock {
    $fixture = New-DeploymentFixture
    try {
        Set-DeploymentHook -Name 'SupervisorRuntimeProbe' -Body {
            param([Parameter(Mandatory)][string]$SupervisorPath)
            if (-not (Test-Path -LiteralPath $SupervisorPath -PathType Leaf)) {
                throw 'The installed supervisor is missing.'
            }
            $probeLock = New-DeploymentLock
            try { throw 'probe-lock-released-stop-before-prompt' }
            finally { $probeLock.Dispose() }
        }
        Assert-DeploymentThrows {
            & (Join-Path $repoRoot 'scripts\Install-Tessera.ps1') -PackagePath $fixture.PackagePath -EnableShell -NoStartMenuShortcut
        } 'probe-lock-released-stop-before-prompt' 'The actual installer releases the recovery lock before GUI verification.'
        Assert-DeploymentFalse (Get-FixtureShellValue $fixture).Present 'A failed probe does not change the shell.'
        Assert-DeploymentEqual (Get-FixtureRecoveryFields $fixture).Count 0 'A failed probe publishes no active registry backup.'
    } finally {
        Remove-DeploymentFixture $fixture
    }
}

# --------------------------------------------------------------------- main

foreach ($test in @(
    'Test-PackageValidation',
    'Test-ActivationAndRestoreWithoutShellValue',
    'Test-ActivationAndRestoreWithStringAndExpandString',
    'Test-IdempotentRerunAndForeignShellRefusal',
    'Test-TypedOwnershipOfForeignExpandString',
    'Test-RestoreRefusesForeignDeletion',
    'Test-StrictRecoveryStateDecoding',
    'Test-RollbackOnOverrideFailure',
    'Test-UnsupportedActivationHasNoSideEffects',
    'Test-PolicyShellIsRefused',
    'Test-SpaceAndUnicodeInstallDirectory',
    'Test-DeploymentLockContention',
    'Test-MalformedBuildInfoIsRefused',
    'Test-PreflightGateAndProbeHook',
    'Test-SmartAppControlStateMapping',
    'Test-EntrypointWhatIfChangesNothing',
    'Test-WhatIfChangesNothing',
    'Test-InstallerProbeDoesNotHoldRecoveryLock'
)) {
    & $test
}

Write-Host ''
if ($script:Failures.Count -eq 0) {
    Write-Host "All $script:PassCount deployment assertions passed."
    exit 0
}
foreach ($failure in $script:Failures) { Write-Host "FAIL: $failure" }
exit 1
