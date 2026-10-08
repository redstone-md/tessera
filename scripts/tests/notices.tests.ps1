# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
# Pure filesystem fixtures; run with pwsh -NoProfile -File scripts/tests/notices.tests.ps1.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Import-Module (Join-Path $repoRoot 'scripts/Tessera.Notices.psm1') -Force
$script:PassCount = 0
$script:Failures = @()

function Assert-NoticeTrue($Condition, [string]$Message) {
    if ($Condition) { $script:PassCount++ } else { $script:Failures += $Message }
}

function Assert-NoticeThrows([scriptblock]$Body, [string]$Pattern, [string]$Message) {
    $errorText = $null
    try { & $Body } catch { $errorText = $_.Exception.Message }
    Assert-NoticeTrue ($null -ne $errorText -and $errorText -match $Pattern) $Message
}

function Write-NoticeFixtureFile([string]$Root, [string]$Relative, [string]$Text) {
    $path = Join-Path $Root $Relative
    New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
    [IO.File]::WriteAllText($path, $Text)
}

function Test-CorrespondingSourceNotices {
    $fixture = Join-Path ([IO.Path]::GetTempPath()) ('tessera-notices-test-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $fixture | Out-Null
    try {
        $source = Join-Path $fixture 'source'
        $vendor = Join-Path $source 'vendor'
        $destination = Join-Path $fixture 'notices.txt'
        $noticeFiles = [ordered]@{
            'vendor/ordinary/LICENSE-MIT' = "Ordinary MIT notice`r`nCopyright retained exactly."
            'vendor/ordinary/Apache-2.0.txt' = 'Ordinary Apache text'
            'crates/tessera-ui/assets/font/OFL.txt' = 'Font notice'
            'crates/tessera-ui/assets/icon.svg.license' = "SPDX-License-Identifier: MIT`nSidecar notice"
            'third-party/i-slint-core/LICENSES/GPL-3.0-only.txt' = 'Full GPL fixture text'
            'third-party/i-slint-core/LICENSES/LicenseRef-Slint-commercial.md' = 'Retained upstream commercial reference text'
            'third-party/i-slint-core/LICENSES/unusual-name' = 'All LICENSES basenames are carried'
            'third-party/i-slint-core/model/repeater.rs.license' = 'SPDX-License-Identifier: GPL-3.0-only'
        }
        foreach ($entry in $noticeFiles.GetEnumerator()) { Write-NoticeFixtureFile $source $entry.Key $entry.Value }
        Write-NoticeFixtureFile $source 'third-party/i-slint-core/Cargo.toml' 'fixture manifest'
        Write-NoticeFixtureFile $source 'third-party/i-slint-core/model/repeater.rs' 'RUST_SOURCE_MUST_NOT_APPEAR'
        Write-NoticeFixtureFile $source 'target/cache/LICENSE' 'INCIDENTAL_CACHE_MUST_NOT_APPEAR'
        Write-NoticeFixtureFile $fixture 'machine-cache/LICENSE' 'MACHINE_CACHE_MUST_NOT_APPEAR'
        $patch = [pscustomobject]@{
            'upstream-revision' = ('1' * 40)
            'upstream-archive-sha256' = ('2' * 64)
            'modified-date' = '2026-10-08'
            'modified-files' = @('Cargo.toml', 'model/repeater.rs')
            'additional-receipt-field' = 'Future metadata retained'
        }
        $core = [pscustomobject]@{
            name = 'i-slint-core'; version = '1.18.1'; license = 'GPL-3.0-only'
            manifest_path = (Join-Path $source 'third-party/i-slint-core/Cargo.toml')
            metadata = [pscustomobject]@{ 'tessera-source-patch' = $patch }
        }
        $packages = @(
            [pscustomobject]@{ name = 'z-build'; version = '9'; license = $null; metadata = $null },
            $core,
            [pscustomobject]@{ name = 'a-opaque'; version = '2'; license = 'LicenseRef-Opaque AND MIT'; metadata = [pscustomobject]@{} }
        )
        Write-TesseraThirdPartyNotices -SourcePath $source -VendorPath $vendor -DestinationPath $destination -DependencyPackages $packages
        $text = [IO.File]::ReadAllText($destination)
        foreach ($entry in $noticeFiles.GetEnumerator()) {
            Assert-NoticeTrue ($text.Contains("----- $($entry.Key) -----")) "Carries archive-relative header: $($entry.Key)"
            Assert-NoticeTrue ($text.Contains($entry.Value)) "Preserves exact notice text: $($entry.Key)"
        }
        $headers = @([regex]::Matches($text, '(?m)^----- (.+) -----\r?$') | ForEach-Object { $_.Groups[1].Value })
        $expected = @($noticeFiles.Keys)
        [Array]::Sort($expected, [StringComparer]::Ordinal)
        Assert-NoticeTrue (($headers -join '|') -ceq ($expected -join '|')) 'Notice headers are ordinal sorted and unique'
        Assert-NoticeTrue ($text.IndexOf('a-opaque 2 | LicenseRef-Opaque AND MIT') -lt $text.IndexOf('i-slint-core 1.18.1 | GPL-3.0-only')) 'Inventory is sorted and preserves opaque license metadata'
        Assert-NoticeTrue ($text.Contains('z-build 9 | See license files in corresponding source')) 'Missing license uses corresponding-source fallback'
        foreach ($receipt in @(
            'Source-patch receipt: i-slint-core 1.18.1',
            'Source manifest: third-party/i-slint-core/Cargo.toml',
            'License choice: GPL-3.0-only',
            ('upstream-revision: ' + ('1' * 40)),
            ('upstream-archive-sha256: ' + ('2' * 64)),
            'modified-date: 2026-10-08',
            'modified-files: Cargo.toml, model/repeater.rs',
            'additional-receipt-field: Future metadata retained',
            'Retained upstream LicenseRef text is provenance only; it does not grant commercial eligibility.'
        )) { Assert-NoticeTrue ($text.Contains($receipt)) "Carries dynamic receipt: $receipt" }
        Assert-NoticeTrue (-not $text.Contains($fixture)) 'No machine-local paths leak into notices'
        Assert-NoticeTrue ($text -notmatch 'RUST_SOURCE_MUST_NOT_APPEAR|INCIDENTAL_CACHE_MUST_NOT_APPEAR|MACHINE_CACHE_MUST_NOT_APPEAR') 'No incidental source or cache files copied'
        # Overlap the vendor argument with the fixed third-party root to exercise deduplication.
        Write-TesseraThirdPartyNotices -SourcePath $source -VendorPath (Join-Path $source 'third-party') -DestinationPath $destination -DependencyPackages $packages
        $overlap = [IO.File]::ReadAllText($destination)
        Assert-NoticeTrue ([regex]::Matches($overlap, '----- third-party/i-slint-core/LICENSES/GPL-3\.0-only\.txt -----').Count -eq 1) 'Overlapping roots do not duplicate notices'
        Assert-NoticeThrows {
            Write-TesseraThirdPartyNotices -SourcePath $source -VendorPath (Join-Path $fixture 'machine-cache') -DestinationPath $destination -DependencyPackages $packages
        } 'inside the corresponding-source' 'External machine cache cannot become a notice root'
        $patch.'upstream-revision' = ''
        Assert-NoticeThrows {
            Write-TesseraThirdPartyNotices -SourcePath $source -VendorPath $vendor -DestinationPath $destination -DependencyPackages $packages
        } 'Incomplete source-patch receipt' 'An explicitly patched dependency cannot silently omit its receipt'
        $patch.'upstream-revision' = ('1' * 40)
        Remove-Item -LiteralPath (Join-Path $source 'third-party/i-slint-core/model/repeater.rs')
        Assert-NoticeThrows {
            Write-TesseraThirdPartyNotices -SourcePath $source -VendorPath $vendor -DestinationPath $destination -DependencyPackages $packages
        } 'missing modified file' 'Receipt must refer to shipped modified files'
        $minimal = Join-Path $fixture 'minimal-source'
        New-Item -ItemType Directory -Path $minimal | Out-Null
        Write-TesseraThirdPartyNotices -SourcePath $minimal -VendorPath (Join-Path $minimal 'vendor') -DestinationPath $destination -DependencyPackages @()
        Assert-NoticeTrue ([IO.File]::ReadAllText($destination).Contains('Dependency inventory')) 'Missing optional roots and empty inventory are supported'
    } finally {
        Remove-Item -LiteralPath $fixture -Recurse -Force
    }
}

Test-CorrespondingSourceNotices
if ($script:Failures.Count -eq 0) {
    Write-Host "All $script:PassCount notice assertions passed."
    exit 0
}
foreach ($failure in $script:Failures) { Write-Host "FAIL: $failure" }
exit 1
