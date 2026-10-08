# Stage Microsoft's pinned ConPTY runtime beside the Devboule daemon.
#
# Downloads the .nupkg pinned in packaging/windows/conpty.json, verifies the
# package SHA-256, extracts only the pinned members, and verifies each one's
# SHA-256, PE machine type and Microsoft Authenticode signature. Everything
# is staged into a temporary directory inside the target and placed only
# after every check has passed — as the conpty\ subdirectory holding
# conpty.dll and both architecture OpenConsole.exe hosts, plus the
# THIRD-PARTY-NOTICES\ directory. The target's previous conpty\ and
# THIRD-PARTY-NOTICES\ are removed before the download begins, so a run that
# fails or is skipped leaves no stale bundle for a later build to ship.
# The download is the only network access this script performs. The Windows
# installer build (pnpm build:installer) runs it; dev builds and the gate do
# not — see docs/conpty-windows.md.
#
# The pin values and the verify-then-stage order are taken from herdr's
# packaging scripts (see NOTICE).

param(
    # Directory that receives the conpty\ bundle (conpty.dll plus the x64 and
    # arm64 OpenConsole.exe hosts) and the THIRD-PARTY-NOTICES\ directory. It
    # must already exist: the daemon executable is expected to live there,
    # because the loader looks only in the executable's own directory, and
    # this script validates the directory before creating anything inside it.
    # Defaults to the repo's target\debug, where a plain cargo build puts
    # devboule-daemon.exe.
    [string]$TargetDir,

    # Verify this local copy of the pinned .nupkg instead of downloading it.
    [string]$PackagePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Get-PeMachine {
    param([string]$Path)
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 0x40 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "not a PE image: $Path"
    }
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    if ($peOffset -lt 0 -or ($peOffset + 6) -gt $bytes.Length -or
        $bytes[$peOffset] -ne 0x50 -or $bytes[$peOffset + 1] -ne 0x45 -or
        $bytes[$peOffset + 2] -ne 0 -or $bytes[$peOffset + 3] -ne 0) {
        throw "invalid PE header: $Path"
    }
    [BitConverter]::ToUInt16($bytes, $peOffset + 4)
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$metadataPath = Join-Path $repoRoot "packaging\windows\conpty.json"
if (-not (Test-Path $metadataPath)) {
    throw "missing pin file: $metadataPath"
}
$metadata = Get-Content $metadataPath -Raw | ConvertFrom-Json
$package = $metadata.package
$pins = $metadata.bundles.x86_64.files

if (-not $TargetDir) {
    $TargetDir = Join-Path $repoRoot "target\debug"
}
# The target is caller-supplied and this script deletes recursively inside
# it, so validate before creating anything there: it must exist, it must not
# be a drive root, and neither it nor either destination may be a reparse
# point — the same distrust of reparse points the loader applies to the
# bundle it loads.
if (-not (Test-Path -LiteralPath $TargetDir -PathType Container)) {
    throw "target directory does not exist: $TargetDir"
}
$targetItem = Get-Item -LiteralPath $TargetDir -Force
if ($targetItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
    throw "target directory is a reparse point: $TargetDir"
}
if ($targetItem.FullName -eq [IO.Path]::GetPathRoot($targetItem.FullName)) {
    throw "refusing to stage into a drive root: $TargetDir"
}
# Delete-first: the previous bundle is removed before the download or any
# verification starts, so every later failure leaves the target without a
# conpty\ or THIRD-PARTY-NOTICES\ tree and the next bundle step fails on the
# missing resource instead of shipping stale files. A destination that is a
# reparse point is refused, not followed — deleting through one could take a
# link target's contents with it.
foreach ($name in @("conpty", "THIRD-PARTY-NOTICES")) {
    $destination = Join-Path $TargetDir $name
    if (Test-Path -LiteralPath $destination) {
        if ((Get-Item -LiteralPath $destination -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "destination is a reparse point: $destination"
        }
        Remove-Item -LiteralPath $destination -Recurse -Force
    }
}
# The staging directory lives under the target so the final placement is a
# real move on one volume; the download may live on the system temp.
New-Item -ItemType Directory -Path $TargetDir -Force | Out-Null
$staging = Join-Path $TargetDir (".conpty-stage-" + [Guid]::NewGuid().ToString("N"))

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("conpty-stage-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot | Out-Null

try {
    if ($PackagePath) {
        if (-not (Test-Path $PackagePath)) {
            throw "package not found: $PackagePath"
        }
        $packageFile = $PackagePath
    }
    else {
        $packageFile = Join-Path $tempRoot "$($package.id).nupkg"
        Write-Host "Downloading $($package.url)"
        Invoke-WebRequest -Uri $package.url -OutFile $packageFile -UseBasicParsing
    }

    $packageHash = (Get-FileHash -Path $packageFile -Algorithm SHA256).Hash
    if ($packageHash -ne $package.sha256) {
        throw ("package SHA-256 mismatch: expected {0}, got {1}" -f $package.sha256, $packageHash)
    }
    Write-Host "verified package  sha256=$packageHash"

    New-Item -ItemType Directory -Path $staging | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($packageFile)
    try {
        foreach ($pin in $pins) {
            $entry = $archive.Entries | Where-Object { $_.FullName -eq $pin.source }
            if (-not $entry) {
                throw "package is missing pinned member $($pin.source)"
            }
            $destination = Join-Path $staging ($pin.destination -replace "/", "\")
            $parent = Split-Path -Parent $destination
            if ($parent) {
                New-Item -ItemType Directory -Path $parent -Force | Out-Null
            }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $destination, $true)

            $fileHash = (Get-FileHash -Path $destination -Algorithm SHA256).Hash
            if ($fileHash -ne $pin.sha256) {
                throw ("SHA-256 mismatch for {0}: expected {1}, got {2}" -f $pin.source, $pin.sha256, $fileHash)
            }
            $machine = Get-PeMachine $destination
            if ($machine -ne [Convert]::ToUInt32($pin.pe_machine, 16)) {
                throw ("PE machine mismatch for {0}: expected {1}, got 0x{2:X4}" -f $pin.source, $pin.pe_machine, $machine)
            }
            $signature = Get-AuthenticodeSignature $destination
            $subject = ""
            if ($signature.SignerCertificate) {
                $subject = $signature.SignerCertificate.Subject
            }
            if ($signature.Status -ne "Valid" -or $subject -notlike "*Microsoft Corporation*") {
                throw ("Authenticode signature is not Microsoft's for {0}: {1} {2}" -f $pin.source, $signature.Status, $subject)
            }
            Write-Host ("verified {0}  sha256={1}" -f $pin.source, $fileHash)
        }
    }
    finally {
        $archive.Dispose()
    }

    # The conpty.dll shim must match the daemon's architecture; the pinned
    # package ships an x64 shim only, so an ARM64 host stages an OpenConsole
    # that matches the kernel while the shim stays usable only under x64
    # emulation of the daemon.
    switch ($env:PROCESSOR_ARCHITECTURE) {
        "AMD64" { $hostArch = "x64" }
        "ARM64" {
            $hostArch = "arm64"
            Write-Warning "ARM64 host: the pinned conpty.dll is the x64 shim; it loads only into an x64 daemon"
        }
        default { throw "unsupported OS architecture: $env:PROCESSOR_ARCHITECTURE" }
    }

    # Microsoft's MIT condition: the licence and the package notice ship with
    # the binaries, in the THIRD-PARTY-NOTICES layout herdr's installer
    # requires. Their bytes are printed, not pinned — the repo's
    # normalization of line endings makes herdr's hashes incomparable.
    $notices = Join-Path $staging "THIRD-PARTY-NOTICES"
    New-Item -ItemType Directory -Path $notices | Out-Null
    Copy-Item (Join-Path $repoRoot "packaging\windows\licenses\Microsoft.Windows.Console.ConPTY-LICENSE.txt") $notices
    Copy-Item (Join-Path $repoRoot "packaging\windows\licenses\Microsoft.Windows.Console.ConPTY-NOTICE.md") $notices
    foreach ($name in @("Microsoft.Windows.Console.ConPTY-LICENSE.txt", "Microsoft.Windows.Console.ConPTY-NOTICE.md")) {
        $hash = (Get-FileHash -Path (Join-Path $notices $name) -Algorithm SHA256).Hash
        Write-Host ("staged THIRD-PARTY-NOTICES/{0}  sha256={1}" -f $name, $hash)
    }

    New-Item -ItemType Directory -Path $TargetDir -Force | Out-Null
    # Placement order is the safety property: the only possible partial
    # placement is notices without binaries, which is inert — the loader
    # stays on the inbox — never a working bundle without its licence. A
    # destination that exists again here (recreated since the delete-first
    # above) is a race or a plant: refuse it rather than move into it, and
    # refuse a reparse point rather than follow it.
    foreach ($name in @("THIRD-PARTY-NOTICES", "conpty")) {
        $destination = Join-Path $TargetDir $name
        if (Test-Path -LiteralPath $destination) {
            if ((Get-Item -LiteralPath $destination -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "destination is a reparse point: $destination"
            }
            throw "destination reappeared after delete-first: $destination"
        }
    }
    Move-Item $notices (Join-Path $TargetDir "THIRD-PARTY-NOTICES")
    Move-Item (Join-Path $staging "conpty") (Join-Path $TargetDir "conpty")
    Write-Host ("Staged conpty\conpty.dll, conpty\{0}\OpenConsole.exe, and THIRD-PARTY-NOTICES into {1}" -f $hostArch, $TargetDir)
}
finally {
    foreach ($temp in @($tempRoot, $staging)) {
        if ($temp -and (Test-Path -LiteralPath $temp)) {
            try {
                Remove-Item -Recurse -Force $temp
            }
            catch {
                Write-Warning "could not clean up $($temp): $($_.Exception.Message)"
            }
        }
    }
}
