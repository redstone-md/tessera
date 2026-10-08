# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Collect only corresponding-source notices; Cargo's machine-local cache is not a notice root.
function Write-TesseraThirdPartyNotices {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$SourcePath,
        [Parameter(Mandatory)][string]$VendorPath,
        [Parameter(Mandatory)][string]$DestinationPath,
        [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$DependencyPackages
    )

    $source = [IO.Path]::GetFullPath($SourcePath)
    $packages = @($DependencyPackages | Sort-Object name, version)
    $notices = @(
        'Dependency inventory; full source and license files are included in the matching source archive.',
        'This inventory includes build/test and other-platform dependencies, not only runtime imports.',
        ''
    )
    $notices += $packages | ForEach-Object {
        $license = if ($_.license) { $_.license } else { 'See license files in corresponding source' }
        "$($_.name) $($_.version) | $license"
    }

    foreach ($package in $packages) {
        $metadataProperty = $package.PSObject.Properties['metadata']
        if ($null -eq $metadataProperty -or $null -eq $metadataProperty.Value) { continue }
        $patchProperty = $metadataProperty.Value.PSObject.Properties['tessera-source-patch']
        if ($null -eq $patchProperty) { continue }
        $patch = $patchProperty.Value
        foreach ($field in 'upstream-revision', 'upstream-archive-sha256', 'modified-date', 'modified-files') {
            if ($null -eq $patch -or $null -eq $patch.PSObject.Properties[$field] -or -not $patch.$field) {
                throw "Incomplete source-patch receipt for $($package.name): $field"
            }
        }
        if ($patch.'upstream-revision' -notmatch '^[0-9a-fA-F]{40}$' -or
            $patch.'upstream-archive-sha256' -notmatch '^[0-9a-fA-F]{64}$' -or
            $patch.'modified-date' -notmatch '^\d{4}-\d{2}-\d{2}$' -or -not $package.license) {
            throw "Invalid source-patch receipt or missing license choice for $($package.name)"
        }
        $manifest = [IO.Path]::GetRelativePath($source, $package.manifest_path).Replace('\', '/')
        if ([IO.Path]::IsPathRooted($manifest) -or $manifest -match '^\.\.(/|$)' -or
            -not (Test-Path -LiteralPath $package.manifest_path -PathType Leaf)) {
            throw "Patched dependency is missing from corresponding source: $($package.name)"
        }
        $packageRoot = Split-Path -Parent $package.manifest_path
        foreach ($file in @($patch.'modified-files')) {
            if ($file -isnot [string] -or [string]::IsNullOrWhiteSpace($file) -or
                [IO.Path]::IsPathRooted($file) -or $file -match '(^|[/\\])\.\.([/\\]|$)' -or
                -not (Test-Path -LiteralPath (Join-Path $packageRoot $file) -PathType Leaf)) {
                throw "Invalid or missing modified file in source-patch receipt for $($package.name): $file"
            }
        }
        $notices += @(
            '',
            "Source-patch receipt: $($package.name) $($package.version)",
            "Source manifest: $manifest",
            "License choice: $($package.license)"
        )
        $notices += $patch.PSObject.Properties | Sort-Object Name | ForEach-Object {
            "$($_.Name): $(@($_.Value) -join ', ')"
        }
        $notices += 'Retained upstream LicenseRef text is provenance only; it does not grant commercial eligibility.'
    }

    $files = [Collections.Generic.SortedDictionary[string, string]]::new([StringComparer]::Ordinal)
    foreach ($root in @($VendorPath, (Join-Path $source 'crates/tessera-ui/assets'), (Join-Path $source 'third-party'))) {
        $relativeRoot = [IO.Path]::GetRelativePath($source, [IO.Path]::GetFullPath($root)).Replace('\', '/')
        if ([IO.Path]::IsPathRooted($relativeRoot) -or $relativeRoot -match '^\.\.(/|$)') {
            throw 'Notice roots must be inside the corresponding-source archive.'
        }
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { continue }
        Get-ChildItem -LiteralPath $root -Recurse -File | ForEach-Object {
            $relative = [IO.Path]::GetRelativePath($source, $_.FullName).Replace('\', '/')
            if ($_.Name -match '^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE)([._-].*)?$' -or
                $_.Name -match '^(OFL|Apache-2\.0|MIT)\.txt$' -or
                $_.Name -match '\.license$' -or $relative -match '(^|/)LICENSES/') {
                $files[$relative] = $_.FullName
            }
        }
    }
    $notices | Set-Content -LiteralPath $DestinationPath -Encoding utf8
    foreach ($entry in $files.GetEnumerator()) {
        @('', "----- $($entry.Key) -----", (Get-Content -LiteralPath $entry.Value -Raw)) |
            Add-Content -LiteralPath $DestinationPath -Encoding utf8
    }
}

Export-ModuleMember -Function Write-TesseraThirdPartyNotices
