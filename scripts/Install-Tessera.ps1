# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
#
# Installs the Tessera alpha package for the current user and, only with the
# explicit -EnableShell switch, makes the packaged shell supervisor the
# per-user sign-in shell (experimental). Windows 11 x64 client editions only.
# Compatible with Windows PowerShell 5.1 and PowerShell 7.
#
# The shell override takes effect at the next sign-in and is reversible with
# Restore-Tessera.ps1. This script never deletes user data, never elevates,
# and never touches the machine Winlogon configuration.

[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory)][string]$PackagePath,
    [switch]$EnableShell,
    [switch]$NoStartMenuShortcut
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($null -eq $env:OS -or $env:OS -ne 'Windows_NT') {
    throw 'Tessera deployment is only supported on Windows.'
}
if ([IntPtr]::Size -ne 8) {
    throw 'Use the 64-bit (x64) Windows PowerShell or PowerShell 7 to install Tessera.'
}

# Optional convenience shortcut via the documented WScript.Shell COM API.
# Never a Run key or startup task; the user starts the installed GUI.
function New-TesseraStartMenuShortcut {
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string]$TargetPath,
        [Parameter(Mandatory)][string]$Version
    )
    if (-not (Test-Path -LiteralPath $TargetPath -PathType Leaf)) {
        throw "The GUI executable is missing: $TargetPath"
    }
    $programs = [Environment]::GetFolderPath('Programs')
    if ([string]::IsNullOrWhiteSpace($programs)) {
        Write-Verbose 'The Start menu folder could not be resolved; skipping the optional shortcut.'
        return
    }
    $linkPath = Join-Path $programs 'Tessera\Tessera.lnk'
    if ($WhatIfPreference) {
        Write-Host "WHATIF: Would create the Start menu shortcut $linkPath"
        return
    }
    if (-not (Test-Path -LiteralPath (Split-Path -Parent $linkPath) -PathType Container)) {
        New-Item -ItemType Directory -Path (Split-Path -Parent $linkPath) -Force | Out-Null
    }
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($linkPath)
        try {
            $shortcut.TargetPath = $TargetPath
            $shortcut.WorkingDirectory = Split-Path -Parent $TargetPath
            $shortcut.Description = "Tessera $Version"
            $shortcut.Save()
        } finally {
            [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shortcut)
        }
    } finally {
        [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
    }
    Write-Verbose "Created the Start menu shortcut $linkPath"
}

$modulePath = Join-Path $PSScriptRoot 'Tessera.Deployment.psm1'
# No -Force: reloading would discard the module's session state (including
# the recovery context another invocation in this session has configured).
Import-Module $modulePath

$package = Test-TesseraPackage -PackagePath $PackagePath

if ($EnableShell) {
    # Preflight is read-only and runs for -WhatIf too, so the plan reports
    # exactly what a real run would decide.
    $support = Test-ShellSupport
    if (-not $support.Supported) {
        Write-Host 'Shell activation is not supported on this host:'
        foreach ($problem in $support.Problems) { Write-Host "  - $problem" }
        Write-Host 'The package can still be installed without the shell override.'
        throw 'Shell activation was refused; rerun without -EnableShell to install the package only.'
    }
}

$confirmation = 'Install the Tessera package for the current user?'
if ($EnableShell) {
    $confirmation = 'Install Tessera AND set it as the per-user sign-in shell (experimental; reversible with Restore-Tessera.ps1)?'
}
# -WhatIf never creates the deployment lock or any file/registry state; it
# prints the read-only plan and stops.
if ($WhatIfPreference) {
    Write-Host "WHATIF: Would validate and copy the package from $($package.PackagePath)"
    Write-Host "WHATIF: Would install to $(Join-Path (Get-TesseraInstallRoot) $package.Version)"
    Write-Host "WHATIF: Would publish recovery scripts to $(Get-TesseraRecoveryDirectory)"
    if (-not $NoStartMenuShortcut) {
        Write-Host 'WHATIF: Would create the Start menu shortcut'
    }
    if ($EnableShell) {
        Write-Host "WHATIF: Would set the per-user Winlogon Shell value to $(Join-Path (Get-TesseraInstallRoot) "$($package.Version)\tessera-shell.exe") at the next sign-in"
    }
    return
}

# Never hold the recovery lock while waiting for interactive confirmation
# or GUI readiness: an existing supervised session may need it to roll back.
if (-not $PSCmdlet.ShouldProcess($package.PackagePath, $confirmation)) {
    Write-Host 'Installation cancelled.'
    return
}
$lock = New-DeploymentLock
try {
    $installDirectory = Copy-TesseraPackage -PackagePath $package.PackagePath -Version $package.Version
    Publish-RecoveryRuntime -SourceDirectory $package.PackagePath
    if (-not $NoStartMenuShortcut) {
        New-TesseraStartMenuShortcut -TargetPath (Join-Path $installDirectory 'Tessera.exe') -Version $package.Version
    }
} finally {
    if ($null -ne $lock) { $lock.Dispose() }
}

if ($EnableShell) {
    # Launches and reaps only the diagnostic GUI; no registry or Explorer changes.
    Write-Host 'Verifying the installed supervisor runtime (tessera-shell.exe --verify-runtime)...'
    Invoke-SupervisorRuntimeVerification -SupervisorPath (Join-Path $installDirectory 'tessera-shell.exe')
    $yes = [System.Management.Automation.Host.ChoiceDescription]::new('&Yes')
    $no = [System.Management.Automation.Host.ChoiceDescription]::new('&No')
    $answer = $Host.UI.PromptForChoice(
        'Enable the Tessera shell',
        'This sets the per-user Winlogon Shell value for the next sign-in. Keep the independent recovery script and a VM snapshot. Continue?',
        [System.Management.Automation.Host.ChoiceDescription[]]@($yes, $no),
        1)
    if ($answer -ne 0) {
        Write-Host 'Shell activation cancelled; the package remains installed.'
        return
    }
    $lock = New-DeploymentLock
    try {
        # Recheck current policy/shell state under the lock after the prompt;
        # an earlier preflight is not authorization to ignore later changes.
        Invoke-ShellActivation -InstallDirectory $installDirectory
    } finally {
        if ($null -ne $lock) { $lock.Dispose() }
    }
    Write-Host 'Shell activation recorded. The override takes effect at your next sign-in.'
    Write-Host "Recovery script: $(Join-Path (Get-TesseraRecoveryDirectory) 'Restore-Tessera.ps1')"
    Write-Host 'If the supervisor cannot run, use the independent script or the'
    Write-Host 'conditional Task Manager/reg.exe emergency steps in START-HERE.txt.'
} else {
    Write-Host "Tessera $($package.Version) installed to $installDirectory"
    Write-Host "Start it with $(Join-Path $installDirectory 'Tessera.exe')"
}
