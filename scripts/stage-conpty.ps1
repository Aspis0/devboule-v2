# Stage Microsoft's pinned ConPTY runtime beside the Devboule daemon.
#
# Downloads the .nupkg pinned in packaging/windows/conpty.json, verifies the
# package SHA-256, extracts only the pinned members, and verifies each one's
# SHA-256, PE machine type and Microsoft Authenticode signature. Everything
# is staged into a temporary directory inside the target and placed only
# after every check has passed — as the conpty\ subdirectory holding
# conpty.dll and both architecture OpenConsole.exe hosts, plus the
# THIRD-PARTY-NOTICES\ directory, replacing any previous bundle by renaming
# it aside and deleting the renamed copy only once the new tree is in place.
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
    # Re-staging replaces the previous bundle without destroying anything it
    # could not replace: the old directories are renamed aside first — a
    # rename of a held bundle (a daemon running from the target) fails
    # atomically, leaving the old bundle intact — the new tree is placed, and
    # only then are the renamed copies deleted. Any failure in between puts
    # the renamed copies back, so the target is exactly as it was. The order
    # of the rename loop is irrelevant; the order of the two moves below is
    # the safety property: the only possible partial placement is notices
    # without binaries, which is inert — the loader stays on the inbox —
    # never a working bundle without its licence.
    $aside = @()
    try {
        foreach ($name in @("THIRD-PARTY-NOTICES", "conpty")) {
            $destination = Join-Path $TargetDir $name
            if (Test-Path -LiteralPath $destination -PathType Container) {
                if ((Get-Item -LiteralPath $destination -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                    throw "destination is a reparse point: $destination"
                }
                $asidePath = Join-Path $TargetDir (".conpty-old-" + [Guid]::NewGuid().ToString("N"))
                try {
                    Move-Item -LiteralPath $destination -Destination $asidePath
                }
                catch {
                    # The failed rename can leave its destination directory behind.
                    if (Test-Path -LiteralPath $asidePath) {
                        try {
                            Remove-Item -Recurse -Force $asidePath
                        }
                        catch {
                            Write-Warning "could not clean up $($asidePath): $($_.Exception.Message)"
                        }
                    }
                    throw "could not move the old $name aside; is a daemon running from $TargetDir and holding its files? $($_.Exception.Message)"
                }
                $aside += [pscustomobject]@{ Aside = $asidePath; Original = $destination }
            }
        }
        Move-Item $notices (Join-Path $TargetDir "THIRD-PARTY-NOTICES")
        Move-Item (Join-Path $staging "conpty") (Join-Path $TargetDir "conpty")
    }
    catch {
        # Put the old bundle back, newest rename first. A placement that got
        # as far as creating a new directory at an original path is removed
        # first — restoring onto it would nest the old copy inside the new —
        # so after the restore the target is exactly as it was.
        for ($i = $aside.Count - 1; $i -ge 0; $i--) {
            if (Test-Path -LiteralPath $aside[$i].Original) {
                Remove-Item -Recurse -Force $aside[$i].Original
            }
            if (Test-Path -LiteralPath $aside[$i].Aside) {
                try {
                    Move-Item -LiteralPath $aside[$i].Aside -Destination $aside[$i].Original -Force
                }
                catch {
                    Write-Warning "could not restore $($aside[$i].Aside): $($_.Exception.Message)"
                }
                # A failed rename can leave its empty destination directory behind.
                if ((Test-Path -LiteralPath $aside[$i].Aside) -and
                    (@(Get-ChildItem -LiteralPath $aside[$i].Aside -Recurse -Force).Count -eq 0)) {
                    Remove-Item -Recurse -Force $aside[$i].Aside
                }
            }
        }
        throw
    }
    # The new tree is complete and in place; the renamed copies are now only
    # garbage. A failure here still leaves a working, licensed bundle.
    foreach ($entry in $aside) {
        try {
            Remove-Item -Recurse -Force $entry.Aside
        }
        catch {
            Write-Warning "staged, but could not delete the previous bundle copy at $($entry.Aside)"
        }
    }
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
