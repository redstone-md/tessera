# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.

[CmdletBinding()]
param([Parameter(Mandatory)][string]$Tag)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Alpha packaging requires native Windows and its MSVC/Windows SDK tools.' }
if ($Tag -notmatch '^v\d+\.\d+\.\d+-alpha\.\d+$') { throw 'Only numbered alpha tags may produce unsigned test packages.' }

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}

function Assert-Executable([string]$Path, [int]$Subsystem, [string]$Version) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $pe = [BitConverter]::ToInt32($bytes, 0x3c)
    if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x00004550 -or
        [BitConverter]::ToUInt16($bytes, $pe + 4) -ne 0x8664 -or
        [BitConverter]::ToUInt16($bytes, $pe + 24) -ne 0x20b -or
        [BitConverter]::ToUInt16($bytes, $pe + 24 + 68) -ne $Subsystem) {
        throw "Wrong PE architecture/subsystem: $Path"
    }
    if ((Get-Item -LiteralPath $Path).VersionInfo.ProductVersion -ne $Version) { throw "Missing alpha version resource: $Path" }
    if ((Get-AuthenticodeSignature -LiteralPath $Path).Status -ne 'NotSigned') { throw 'This alpha path expects explicitly unsigned artifacts; signed releases need a separate reviewed process.' }
    $imports = Invoke-Checked $script:Dumpbin @('/nologo', '/DEPENDENTS', $Path)
    if (($imports -join "`n") -match '(?i)(VCRUNTIME\d+|MSVCP\d+|ucrtbase)\S*\.dll') { throw "Unexpected dynamic CRT requirement: $Path" }
    $manifestPath = "$Path.manifest-check.xml"
    Invoke-Checked $script:Mt @('-nologo', "-inputresource:$Path;#1", "-out:$manifestPath")
    [xml]$manifest = Get-Content -LiteralPath $manifestPath -Raw
    $level = $manifest.SelectNodes("//*[local-name()='requestedExecutionLevel']")
    $dpi = $manifest.SelectSingleNode("//*[local-name()='dpiAwareness']")
    if ($level.Count -ne 1 -or $level[0].level -ne 'asInvoker' -or $level[0].uiAccess -ne 'false' -or $null -eq $dpi -or $dpi.InnerText -ne 'PerMonitorV2') {
        throw "Privilege/DPI manifest does not match the alpha baseline: $Path"
    }
    Remove-Item -LiteralPath $manifestPath
}

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    if (@(Invoke-Checked git @('status', '--porcelain')).Count -gt 0) { throw 'Package from a clean tagged checkout only.' }
    $commit = (Invoke-Checked git @('rev-parse', 'HEAD')).Trim()
    $tagCommit = (Invoke-Checked git @('rev-parse', "$Tag^{commit}")).Trim()
    if ($commit -ne $tagCommit) { throw 'The release tag does not point to this checkout.' }
    $metadata = (Invoke-Checked cargo @('+1.92.0', 'metadata', '--format-version', '1', '--no-deps', '--locked') | Out-String | ConvertFrom-Json)
    $version = ($metadata.packages | Where-Object name -eq 'tessera').version
    if ($Tag -ne "v$version") { throw 'Tag and Cargo package versions differ.' }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $script:Dumpbin = Invoke-Checked $vswhere @('-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-find', 'VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe') | Select-Object -Last 1
    $kits = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots').KitsRoot10
    $script:Mt = Get-ChildItem (Join-Path $kits 'bin/*/x64/mt.exe') | Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
    if (-not $script:Dumpbin -or -not $script:Mt) { throw 'MSVC dumpbin and Windows SDK mt are required.' }

    $previousFlags = $env:RUSTFLAGS
    try {
        $env:RUSTFLAGS = '-C target-feature=+crt-static'
        Invoke-Checked cargo @('+1.92.0', 'build', '-p', 'tessera', '--bins', '--release', '--target', 'x86_64-pc-windows-msvc', '--locked')
    } finally { $env:RUSTFLAGS = $previousFlags }

    $output = Join-Path $root "target/alpha/$version"
    if (Test-Path -LiteralPath $output) { throw 'Output already exists; choose a fresh alpha version rather than overwriting release files.' }
    $packageName = "tessera-$version-windows-x64"
    $package = Join-Path $output $packageName
    New-Item -ItemType Directory -Path $package | Out-Null
    Copy-Item 'target/x86_64-pc-windows-msvc/release/tessera-desktop.exe' (Join-Path $package 'Tessera.exe')
    Copy-Item 'target/x86_64-pc-windows-msvc/release/tessera.exe' (Join-Path $package 'tessera-cli.exe')
    Copy-Item 'target/x86_64-pc-windows-msvc/release/tessera-shell.exe' (Join-Path $package 'tessera-shell.exe')
    Assert-Executable (Join-Path $package 'Tessera.exe') 2 $version
    Assert-Executable (Join-Path $package 'tessera-cli.exe') 3 $version
    Assert-Executable (Join-Path $package 'tessera-shell.exe') 2 $version
    $actualVersion = Invoke-Checked (Join-Path $package 'tessera-cli.exe') @('--version')
    if ($actualVersion.Trim() -ne "Tessera $version") { throw 'Packaged CLI version does not match the release.' }
    Invoke-Checked (Join-Path $package 'tessera-cli.exe') @('demo')
    Invoke-Checked (Join-Path $package 'tessera-cli.exe') @('inspect') | Out-Null
    Copy-Item 'LICENSE' (Join-Path $package 'LICENSE.txt')
    Copy-Item 'docs/alpha-testing.md' (Join-Path $package 'START-HERE.txt')
    foreach ($deploymentScript in 'Install-Tessera.ps1', 'Restore-Tessera.ps1', 'Tessera.Deployment.psm1') {
        Copy-Item (Join-Path 'scripts' $deploymentScript) (Join-Path $package $deploymentScript)
    }
    @("Tessera $version", "Source commit: $commit", 'Target: x86_64-pc-windows-msvc', 'Toolchain: Rust 1.92.0', 'Signing: UNSIGNED TEST BUILD', 'CRT: statically linked; system Windows DLLs remain required') | Set-Content (Join-Path $package 'BUILD-INFO.txt') -Encoding utf8

    # Complete corresponding source, including locked dependencies and their notices.
    $source = Join-Path $output "tessera-$version-source"
    $repoArchive = Join-Path $output 'tracked-source.zip'
    Invoke-Checked git @('archive', '--format=zip', "--output=$repoArchive", 'HEAD')
    Expand-Archive -LiteralPath $repoArchive -DestinationPath $source
    Remove-Item -LiteralPath $repoArchive
    $vendor = Join-Path $source 'vendor'
    Invoke-Checked cargo @('+1.92.0', 'vendor', '--locked', $vendor) | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $source '.cargo') -Force | Out-Null
    @('[source.crates-io]', 'replace-with = "vendored-sources"', '[source.vendored-sources]', 'directory = "vendor"') |
        Set-Content (Join-Path $source '.cargo/config.toml') -Encoding utf8
    Push-Location $source
    try {
        Invoke-Checked cargo @('+1.92.0', 'metadata', '--locked', '--offline', '--format-version', '1') | Out-Null
    } finally { Pop-Location }
    $commit | Set-Content (Join-Path $source 'SOURCE-COMMIT.txt') -Encoding utf8
    $dependencies = Invoke-Checked cargo @('+1.92.0', 'metadata', '--locked', '--format-version', '1') | Out-String | ConvertFrom-Json
    $notices = @('Dependency inventory; full source and license files are included in the matching source archive.', 'This inventory includes build/test and other-platform dependencies, not only runtime imports.', '')
    $notices += $dependencies.packages | Sort-Object name, version | ForEach-Object {
        $license = if ($_.license) { $_.license } else { 'See license files in corresponding source' }
        "$($_.name) $($_.version) | $license"
    }
    $noticePath = Join-Path $package 'THIRD-PARTY-NOTICES.txt'
    $notices | Set-Content $noticePath -Encoding utf8
    # Carry actual notices with the binaries, not only SPDX expressions/links.
    Get-ChildItem $vendor -Recurse -File | Where-Object {
        $_.Name -match '^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE)([._-].*)?$' -or
        $_.Name -match '^(OFL|Apache-2\.0|MIT)\.txt$'
    } | Sort-Object FullName | ForEach-Object {
        $relative = [IO.Path]::GetRelativePath($source, $_.FullName)
        @("", "----- $relative -----", (Get-Content -LiteralPath $_.FullName -Raw)) |
            Add-Content $noticePath -Encoding utf8
    }
    # Validate the exact shipped payload only after its required notices are complete.
    Import-Module (Join-Path $package 'Tessera.Deployment.psm1')
    $validated = Test-TesseraPackage -PackagePath $package
    if ($validated.Version -ne $version -or $validated.Commit -ne $commit) { throw 'Deployment metadata does not match the tagged build.' }
    Invoke-SupervisorRuntimeVerification -SupervisorPath $validated.SupervisorPath
    Compress-Archive -Path $package -DestinationPath (Join-Path $output "$packageName.zip") -CompressionLevel Optimal
    Compress-Archive -Path $source -DestinationPath (Join-Path $output "tessera-$version-source.zip") -CompressionLevel Optimal
    Get-ChildItem $output -Filter '*.zip' | Sort-Object Name | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $($_.Name)"
    } | Set-Content (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
    Write-Host "Alpha assets are ready in $output"
} finally { Pop-Location }
