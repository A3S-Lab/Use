[CmdletBinding()]
param(
    [string]$Version = $env:A3S_USE_VERSION,
    [string]$BaseUrl = $env:A3S_USE_RELEASE_BASE_URL,
    [string]$InstallRoot = $env:A3S_USE_INSTALL_ROOT,
    [string]$BinDir = $env:A3S_USE_BIN_DIR,
    [switch]$NoPathUpdate
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repository = 'A3S-Lab/Use'

function Fail([string]$Message) {
    throw "a3s-use installer: $Message"
}

function Assert-DownloadUri([uri]$Uri, [switch]$AllowQuery) {
    if (-not [string]::IsNullOrEmpty($Uri.UserInfo)) {
        Fail 'download URLs cannot contain credentials'
    }
    if ((-not $AllowQuery -and -not [string]::IsNullOrEmpty($Uri.Query)) -or
        -not [string]::IsNullOrEmpty($Uri.Fragment)) {
        Fail 'the download URL contains an unexpected query or fragment'
    }
    if ($Uri.Scheme -eq 'https') {
        return
    }
    if ($Uri.Scheme -eq 'http' -and $Uri.IsLoopback) {
        return
    }
    Fail 'downloads require HTTPS; plain HTTP is allowed only for a loopback test server'
}

function Invoke-ReleaseFileDownload([uri]$Uri, [string]$Destination) {
    $Handler = [Net.Http.HttpClientHandler]::new()
    $Handler.AllowAutoRedirect = $false
    if ($Uri.IsLoopback) {
        $Handler.UseProxy = $false
    }
    $Client = [Net.Http.HttpClient]::new($Handler)
    $Client.DefaultRequestHeaders.UserAgent.ParseAdd('a3s-use-installer/1.0')
    try {
        $CurrentUri = $Uri
        foreach ($Redirect in 0..5) {
            Assert-DownloadUri $CurrentUri -AllowQuery:($Redirect -gt 0)
            $Response = $Client.GetAsync(
                $CurrentUri,
                [Net.Http.HttpCompletionOption]::ResponseHeadersRead
            ).GetAwaiter().GetResult()
            try {
                $Status = [int]$Response.StatusCode
                if ($Status -ge 300 -and $Status -lt 400) {
                    if ($Redirect -eq 5 -or $null -eq $Response.Headers.Location) {
                        Fail 'the release download exceeded the redirect limit'
                    }
                    $CurrentUri = if ($Response.Headers.Location.IsAbsoluteUri) {
                        $Response.Headers.Location
                    } else {
                        [uri]::new($CurrentUri, $Response.Headers.Location)
                    }
                    continue
                }
                $Response.EnsureSuccessStatusCode() | Out-Null
                $InputStream = $Response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
                try {
                    $OutputStream = [IO.File]::Create($Destination)
                    try {
                        $InputStream.CopyTo($OutputStream)
                        $OutputStream.Flush($true)
                    }
                    finally {
                        $OutputStream.Dispose()
                    }
                }
                finally {
                    $InputStream.Dispose()
                }
                return
            }
            finally {
                $Response.Dispose()
            }
        }
        Fail 'the release download exceeded the redirect limit'
    }
    finally {
        $Client.Dispose()
        $Handler.Dispose()
    }
}

function Get-ReleaseFile([uri]$Uri, [string]$Destination) {
    foreach ($Attempt in 1..3) {
        try {
            Invoke-ReleaseFileDownload $Uri $Destination
            return
        }
        catch {
            if ($Attempt -eq 3) {
                throw
            }
            Start-Sleep -Milliseconds (250 * $Attempt)
        }
    }
}

function Assert-AbsolutePath([string]$Value, [string]$Name) {
    if ([string]::IsNullOrWhiteSpace($Value) -or -not [IO.Path]::IsPathRooted($Value)) {
        Fail "$Name must be an absolute path"
    }
    if ($Value.Contains("`r") -or $Value.Contains("`n")) {
        Fail "$Name cannot contain a newline"
    }
}

function Get-ReleaseTreeManifest([string]$Root) {
    $RootPrefix = $Root.TrimEnd('\') + '\'
    @(
        Get-ChildItem -LiteralPath $Root -Recurse -Force | ForEach-Object {
            if (($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                Fail "$Root contains a reparse point: $($_.FullName)"
            }
            if (-not $_.PSIsContainer) {
                $Relative = $_.FullName.Substring($RootPrefix.Length).Replace('\', '/')
                $Hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
                "$Relative`0$($_.Length)`0$Hash"
            }
        } | Sort-Object
    )
}

if ([string]::IsNullOrWhiteSpace($BaseUrl)) {
    $BaseUrl = "https://github.com/$Repository/releases/download"
}
if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        Fail 'LOCALAPPDATA or -InstallRoot is required'
    }
    $InstallRoot = Join-Path $env:LOCALAPPDATA 'A3S\Use'
}
if ([string]::IsNullOrWhiteSpace($BinDir)) {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        Fail 'LOCALAPPDATA or -BinDir is required'
    }
    $BinDir = Join-Path $env:LOCALAPPDATA 'A3S\bin'
}

Assert-AbsolutePath $InstallRoot '-InstallRoot'
Assert-AbsolutePath $BinDir '-BinDir'
if ($InstallRoot.Contains('%')) {
    Fail '-InstallRoot cannot contain % because the managed command shim is a cmd file'
}
$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
$BinDir = [IO.Path]::GetFullPath($BinDir)
$BaseUrl = $BaseUrl.TrimEnd('/')
$BaseUri = [uri]$BaseUrl
Assert-DownloadUri $BaseUri

if ([string]::IsNullOrWhiteSpace($Version)) {
    if ($BaseUrl -ne "https://github.com/$Repository/releases/download") {
        Fail '-Version is required with a custom -BaseUrl'
    }
    $LatestUri = [uri]"https://api.github.com/repos/$Repository/releases/latest"
    Assert-DownloadUri $LatestUri
    $Latest = Invoke-RestMethod -Uri $LatestUri -Headers @{
        Accept = 'application/vnd.github+json'
        'User-Agent' = 'a3s-use-installer/1.0'
    }
    $Version = [string]$Latest.tag_name
}

$Version = $Version.Trim()
if ($Version.StartsWith('v', [StringComparison]::OrdinalIgnoreCase)) {
    $Version = $Version.Substring(1)
}
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z][0-9A-Za-z.-]*)?$') {
    Fail "release version is not a supported semantic version: $Version"
}
$Tag = "v$Version"

if (-not [Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [Runtime.InteropServices.OSPlatform]::Windows
)) {
    Fail 'install.ps1 supports Windows only; use install.sh on Linux or macOS'
}
if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne
    [Runtime.InteropServices.Architecture]::X64) {
    Fail "unsupported Windows architecture: $([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
}

$Platform = 'windows-x86_64'
$ArchiveName = "a3s-use-$Version-$Platform.zip"
$ReleaseUri = "$BaseUrl/$Tag"
$DownloadRoot = Join-Path ([IO.Path]::GetTempPath()) "a3s-use-install-$([guid]::NewGuid().ToString('N'))"
$StageRoot = $null
$InstallLock = $null
$LockPath = $null
$TemporaryShim = $null

New-Item -ItemType Directory -Path $DownloadRoot | Out-Null
try {
    $ChecksumsPath = Join-Path $DownloadRoot 'checksums.txt'
    $ArchivePath = Join-Path $DownloadRoot $ArchiveName
    Get-ReleaseFile ([uri]"$ReleaseUri/checksums.txt") $ChecksumsPath
    Get-ReleaseFile ([uri]"$ReleaseUri/$ArchiveName") $ArchivePath

    $ExpectedMatches = @(
        Get-Content -LiteralPath $ChecksumsPath | ForEach-Object {
            if ($_ -match '^([0-9A-Fa-f]{64})\s+\*?(.+)$' -and $Matches[2] -ceq $ArchiveName) {
                $Matches[1].ToLowerInvariant()
            }
        }
    )
    if ($ExpectedMatches.Count -ne 1) {
        Fail "checksums.txt must contain exactly one entry for $ArchiveName"
    }
    $ExpectedSha256 = $ExpectedMatches[0]
    $ActualSha256 = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualSha256 -cne $ExpectedSha256) {
        Fail "SHA-256 mismatch for $ArchiveName"
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Zip = [IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $SeenEntries = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        foreach ($Entry in $Zip.Entries) {
            $EntryPath = $Entry.FullName.Replace('\', '/')
            $Segments = $EntryPath.Split('/', [StringSplitOptions]::RemoveEmptyEntries)
            if (
                [IO.Path]::IsPathRooted($EntryPath) -or
                $EntryPath.StartsWith('/') -or
                $EntryPath.Contains('//') -or
                $Segments -contains '.' -or
                $Segments -contains '..'
            ) {
                Fail "the release archive contains an unsafe path: $($Entry.FullName)"
            }
            foreach ($Segment in $Segments) {
                if ($Segment -match '[<>:"|?*\x00-\x1F]' -or $Segment -match '[ .]$') {
                    Fail "the release archive contains an invalid Windows path: $($Entry.FullName)"
                }
                $Stem = $Segment.Split('.')[0]
                if ($Stem -match '^(?i:con|prn|aux|nul|com[1-9]|lpt[1-9])$') {
                    Fail "the release archive contains a reserved Windows path: $($Entry.FullName)"
                }
            }
            $EntryIdentity = $EntryPath.TrimEnd('/')
            if (-not [string]::IsNullOrEmpty($EntryIdentity) -and -not $SeenEntries.Add($EntryIdentity)) {
                Fail "the release archive contains a duplicate Windows path: $($Entry.FullName)"
            }
            $UnixFileType = (($Entry.ExternalAttributes -shr 16) -band 0xF000)
            if ($UnixFileType -notin @(0, 0x4000, 0x8000)) {
                Fail "the release archive contains a link or special file: $($Entry.FullName)"
            }
        }
    }
    finally {
        $Zip.Dispose()
    }

    $ReleasesRoot = Join-Path $InstallRoot 'releases'
    $ReleaseRoot = Join-Path $ReleasesRoot $Version
    New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
    $InstallRootItem = Get-Item -LiteralPath $InstallRoot -Force
    if (($InstallRootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        Fail 'the installation root cannot be a reparse point'
    }
    $LockPath = Join-Path $InstallRoot '.install.lock'
    try {
        $InstallLock = [IO.File]::Open(
            $LockPath,
            [IO.FileMode]::OpenOrCreate,
            [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::None
        )
    }
    catch {
        Fail "another installation is active: $LockPath"
    }

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $ShimPath = Join-Path $BinDir 'a3s-use.cmd'
    if (Test-Path -LiteralPath $ShimPath) {
        if (-not (Test-Path -LiteralPath $ShimPath -PathType Leaf)) {
            Fail "refusing to replace a non-file command: $ShimPath"
        }
        $ExistingShim = Get-Content -LiteralPath $ShimPath -Raw
        if (-not $ExistingShim.Contains('A3S_USE_MANAGED_SHIM=1')) {
            Fail "refusing to replace a command not managed by A3S Use: $ShimPath"
        }
    }

    New-Item -ItemType Directory -Force -Path $ReleasesRoot | Out-Null
    $ReleasesRootItem = Get-Item -LiteralPath $ReleasesRoot -Force
    if (($ReleasesRootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        Fail 'the releases directory cannot be a reparse point'
    }
    $StageRoot = Join-Path $ReleasesRoot ".stage-$Version-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $StageRoot | Out-Null
    Expand-Archive -LiteralPath $ArchivePath -DestinationPath $StageRoot

    $Executable = Join-Path $StageRoot 'a3s-use.exe'
    $Driver = Join-Path $StageRoot 'a3s-use-browser-driver.exe'
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        Fail 'the release archive does not contain a3s-use.exe'
    }
    if (-not (Test-Path -LiteralPath $Driver -PathType Leaf)) {
        Fail 'the release archive does not contain the Browser driver'
    }
    $RequiredFiles = @(
        'skills/a3s-use-browser/SKILL.md',
        'skill-data/core/SKILL.md',
        'ocr-skills/a3s-use-ocr/SKILL.md',
        'ocr-models/PP-OCRv6_small/det/inference.onnx',
        'ocr-models/PP-OCRv6_small/det/inference.yml',
        'ocr-models/PP-OCRv6_small/rec/inference.onnx',
        'ocr-models/PP-OCRv6_small/rec/inference.yml',
        'dashboard/index.html'
    )
    foreach ($RequiredFile in $RequiredFiles) {
        $RequiredPath = Join-Path $StageRoot $RequiredFile
        if (-not (Test-Path -LiteralPath $RequiredPath -PathType Leaf)) {
            Fail "the release archive is missing required file: $RequiredFile"
        }
    }
    Set-Content -LiteralPath (Join-Path $StageRoot '.a3s-use-archive.sha256') -Value $ExpectedSha256 -Encoding ascii

    if (Test-Path -LiteralPath $ReleaseRoot) {
        $ReleaseRootItem = Get-Item -LiteralPath $ReleaseRoot -Force
        if (($ReleaseRootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Fail "the existing release path cannot be a reparse point: $ReleaseRoot"
        }
        $DigestPath = Join-Path $ReleaseRoot '.a3s-use-archive.sha256'
        $InstalledDigest = if (Test-Path -LiteralPath $DigestPath -PathType Leaf) {
            (Get-Content -LiteralPath $DigestPath -Raw).Trim()
        } else {
            ''
        }
        if ($InstalledDigest -cne $ExpectedSha256) {
            Fail "$ReleaseRoot already exists with different or unverifiable content"
        }
        $ExpectedTree = Get-ReleaseTreeManifest $StageRoot
        $InstalledTree = Get-ReleaseTreeManifest $ReleaseRoot
        if (@(Compare-Object -ReferenceObject $ExpectedTree -DifferenceObject $InstalledTree -CaseSensitive).Count -ne 0) {
            Fail "$ReleaseRoot does not match the verified release archive"
        }
        Remove-Item -LiteralPath $StageRoot -Recurse -Force
        $StageRoot = $null
    }
    else {
        Move-Item -LiteralPath $StageRoot -Destination $ReleaseRoot
        $StageRoot = $null
    }

    $InstalledExecutable = Join-Path $ReleaseRoot 'a3s-use.exe'
    $TemporaryShim = Join-Path $BinDir ".a3s-use-$([guid]::NewGuid().ToString('N')).cmd"
    $OcrHome = Join-Path $ReleaseRoot 'ocr-models'
    $OcrSkills = Join-Path $ReleaseRoot 'ocr-skills'
    $BrowserSkills = Join-Path $ReleaseRoot 'skill-data'
    $ShimContent = @"
@echo off
rem A3S_USE_MANAGED_SHIM=1
if not defined A3S_USE_OCR_HOME set "A3S_USE_OCR_HOME=$OcrHome"
if not defined A3S_USE_OCR_SKILLS_DIR set "A3S_USE_OCR_SKILLS_DIR=$OcrSkills"
if not defined A3S_USE_BROWSER_SKILLS_DIR set "A3S_USE_BROWSER_SKILLS_DIR=$BrowserSkills"
"$InstalledExecutable" %*
"@ -replace "`r?`n", "`r`n"
    [IO.File]::WriteAllText($TemporaryShim, $ShimContent, [Text.UTF8Encoding]::new($false))
    if ([IO.File]::Exists($ShimPath)) {
        [IO.File]::Replace($TemporaryShim, $ShimPath, $null, $true)
    }
    else {
        [IO.File]::Move($TemporaryShim, $ShimPath)
    }
    $TemporaryShim = $null

    if (-not $NoPathUpdate) {
        $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $PathEntries = @($UserPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
        if (-not ($PathEntries | Where-Object { $_.TrimEnd('\') -ieq $BinDir.TrimEnd('\') })) {
            $UpdatedPath = (@($BinDir) + $PathEntries) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $UpdatedPath, 'User')
        }
        if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -ieq $BinDir.TrimEnd('\') })) {
            $env:Path = "$BinDir;$env:Path"
        }
    }

    Write-Output "Installed A3S Use $Version for $Platform"
    Write-Output "Verified archive: sha256:$ExpectedSha256"
    Write-Output "Command: $ShimPath"
    if ($NoPathUpdate) {
        Write-Output "Add $BinDir to PATH to invoke a3s-use directly."
    }
}
finally {
    if ($null -ne $InstallLock) {
        $InstallLock.Dispose()
        if ($null -ne $LockPath -and (Test-Path -LiteralPath $LockPath -PathType Leaf)) {
            Remove-Item -LiteralPath $LockPath -Force
        }
    }
    if ($null -ne $StageRoot -and (Test-Path -LiteralPath $StageRoot)) {
        Remove-Item -LiteralPath $StageRoot -Recurse -Force
    }
    if ($null -ne $TemporaryShim -and (Test-Path -LiteralPath $TemporaryShim -PathType Leaf)) {
        Remove-Item -LiteralPath $TemporaryShim -Force
    }
    if (Test-Path -LiteralPath $DownloadRoot) {
        Remove-Item -LiteralPath $DownloadRoot -Recurse -Force
    }
}
