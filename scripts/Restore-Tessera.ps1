# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
#
# Restores the previously recorded shell configuration after the
# experimental Tessera per-user shell override. This script is published
# next to Tessera.Deployment.psm1 in %LOCALAPPDATA%\Tessera\Recovery and
# must keep working even if the installed Tessera binaries are deleted.
#
# It never deletes user data, never removes the Winlogon key, and by default
# starts the absolute Windows-folder explorer.exe after a successful restore.
# Compatible with Windows PowerShell 5.1 and PowerShell 7.

[CmdletBinding(SupportsShouldProcess)]
param([switch]$NoStartExplorer)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$modulePath = Join-Path $PSScriptRoot 'Tessera.Deployment.psm1'
# No -Force: reloading would discard the module's session state (including
# the recovery context another invocation in this session has configured).
Import-Module $modulePath

if ($null -eq $env:OS -or $env:OS -ne 'Windows_NT') {
    throw 'Tessera recovery is only supported on Windows.'
}
if ([IntPtr]::Size -ne 8) {
    throw 'Use the 64-bit (x64) Windows PowerShell or PowerShell 7 to run recovery.'
}

# -WhatIf never creates the deployment lock or mutates anything; it prints
# the read-only plan and stops.
if ($WhatIfPreference) {
    Write-Host 'WHATIF: Would read the Tessera recovery backup and restore the recorded original shell value at the next sign-in.'
    return
}

$lock = New-DeploymentLock
try {
    $result = Invoke-ShellRestore
    $startExplorer = $false

    switch ($result.Action) {
        'Restored' {
            Write-Host 'The recorded original shell configuration was restored. It takes effect at the next sign-in.'
            $startExplorer = -not $NoStartExplorer
        }
        'AlreadyOriginal' {
            Write-Host 'The shell configuration already equals the recorded original; the active flag was cleared.'
            $startExplorer = -not $NoStartExplorer
        }
        'AlreadyRestored' {
            # The backup exists but is already inactive; a blank current
            # desktop is still helped by launching Explorer.
            Write-Host 'Tessera was not the active shell; nothing to restore.'
            $startExplorer = -not $NoStartExplorer
        }
        'WouldRestore' {
            Write-Host 'WHATIF: Recovery would restore the recorded original shell value.'
            $startExplorer = $false
        }
    }

    Write-Host "The recovery backup was retained at $(Get-TesseraRecoveryDirectory) for evidence."

    if ($startExplorer) {
        $explorer = Join-Path ([Environment]::GetFolderPath('Windows')) 'explorer.exe'
        if (Test-Path -LiteralPath $explorer -PathType Leaf) {
            Write-Host "Starting $explorer"
            $null = Start-Process -FilePath $explorer
        } else {
            Write-Warning "Windows Explorer was not found at $explorer; start it from Task Manager if needed."
        }
    }

    Write-Host ''
    Write-Host 'If the desktop still fails to load at sign-in, use the emergency steps in the'
    Write-Host 'package documentation: delete the Winlogon Shell value with reg.exe and start'
    Write-Host 'explorer.exe from Task Manager.'
} finally {
    if ($null -ne $lock) { $lock.Dispose() }
}
