<#
.SYNOPSIS
    Build a shippable WF64 IDE release folder.

.DESCRIPTION
    Compiles wf64-ui under the `release-ship` profile (fat LTO,
    single codegen unit, stripped, abort-on-panic) with the
    `gui_subsystem` feature so launching from Explorer doesn't pop
    a console window.  Stages the binary + runtime data files
    (kernel/, lib/, demos/, docs/) under `release/wf64/` so the
    folder is double-click-runnable as-is.

    Layout produced:
        release/wf64/
            wf64-ui.exe        - the IDE entry point
            kernel/*.masm      - kernel source (loaded at boot via JIT)
            lib/core.f         - Forth standard library
            demos/*.f          - sample programs reachable via Demos menu
            docs/              - bundled documentation (if present)
            README.txt         - end-user quickstart

.PARAMETER OutDir
    Output folder (default: `release\wf64` under the repo root).

.PARAMETER Zip
    If set, also produces `release\wf64-<version>.zip` alongside
    the staging folder.

.EXAMPLE
    pwsh tools\build-release.ps1
        Builds and stages to release\wf64\

    pwsh tools\build-release.ps1 -Zip
        Same, plus a zipped distribution.
#>
[CmdletBinding()]
param(
    [string]$OutDir,
    [switch]$Zip
)

$ErrorActionPreference = 'Stop'

# Repo root = the parent of tools\, regardless of where we were invoked from.
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $repo

if (-not $OutDir) {
    $OutDir = Join-Path $repo 'release\wf64'
}

Write-Host "[release] repo  = $repo"
Write-Host "[release] out   = $OutDir"
Write-Host ""

#  1. Build the binary 
Write-Host "[release] cargo build --profile release-ship --features gui_subsystem ..."
& cargo build --profile release-ship --features gui_subsystem --bin wf64-ui
if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }

$exe = Join-Path $repo 'target\release-ship\wf64-ui.exe'
if (-not (Test-Path $exe)) {
    throw "expected $exe - cargo built but the file isn't where we look"
}

Write-Host "[release] built  $exe ($([math]::Round((Get-Item $exe).Length / 1MB, 2)) MB)"
Write-Host ""

#  2. Stage the folder 
if (Test-Path $OutDir) {
    Write-Host "[release] cleaning existing $OutDir"
    Remove-Item -Recurse -Force $OutDir
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# Binary
Copy-Item $exe (Join-Path $OutDir 'wf64-ui.exe')
Write-Host "[release] staged wf64-ui.exe"

# Runtime DLLs.  JASM dynamically links LLVM-C.dll; the shipped
# folder must include every DLL the binary depends on or the user
# gets a "missing module" error on launch.  Cargo drops them in
# the same folder as the exe.
$buildDir = Split-Path $exe -Parent
$dlls = Get-ChildItem -Path $buildDir -Filter '*.dll' -File -ErrorAction SilentlyContinue
foreach ($dll in $dlls) {
    Copy-Item $dll.FullName (Join-Path $OutDir $dll.Name)
    $sizeMB = [math]::Round($dll.Length / 1MB, 1)
    Write-Host "[release] staged $($dll.Name) ($sizeMB MB)"
}
if (-not $dlls) {
    Write-Warning "[release] no DLLs alongside the exe - if statically linked, ignore; otherwise the staged folder is missing runtime deps"
}

# Runtime data: kernel + lib + demos.
foreach ($dir in @('kernel', 'lib', 'demos')) {
    $src = Join-Path $repo $dir
    if (Test-Path $src) {
        Copy-Item -Recurse $src (Join-Path $OutDir $dir)
        $count = (Get-ChildItem -Recurse -File (Join-Path $OutDir $dir)).Count
        Write-Host "[release] staged $dir/ ($count files)"
    } else {
        Write-Warning "[release] $dir/ not found in repo - skipping"
    }
}

# Docs - copy only the user-facing docs, not developer/internal files.
$userDocFiles = @(
    'index.md',
    'getting-started.md',
    'forth-tutorial.md',
    'forth-reference.md',
    'ide-guide.md',
    'keyboard-shortcuts.md'
)
$docDest = Join-Path $OutDir 'docs'
New-Item -ItemType Directory -Force -Path $docDest | Out-Null
$repoDocs = Join-Path $repo 'docs'
foreach ($f in $userDocFiles) {
    $src = Join-Path $repoDocs $f
    if (Test-Path $src) {
        Copy-Item $src $docDest
        Write-Host "[release] staged docs/$f"
    } else {
        Write-Warning "[release] docs/$f not found - skipping"
    }
}

# DocCrate viewer no longer staged: Help -> Documentation now opens the
# manual in-window (the doc/help panes render Markdown via the embedded
# docpane core), so the standalone doc-crate.exe is not shipped.

# End-user quickstart.
$readme = @"
WF64 - 64-bit STC Forth IDE
============================

Quick start
-----------
Double-click wf64-ui.exe.  The IDE opens to a Forth console pane.
Type at the > prompt and press Enter:

    : square  dup * ;
    7 square .

Should print 7 ok and 49 ok.

The Demos menu loads ready-to-run example programs.
View menu opens the stack viewer (Ctrl+Shift+K), log pane
(Ctrl+Shift+L), and additional REPL panes (Ctrl+Shift+P).
File -> New (Ctrl+N) opens the Forth source editor.

Help -> Documentation (F1) opens the full user guide in a
document pane inside the IDE - rendered Markdown with a sidebar
to browse pages.  No external viewer required.

Layout
------
    wf64-ui.exe   - the IDE binary (drag a shortcut to your desktop)
    LLVM-C.dll    - LLVM runtime (must stay alongside wf64-ui.exe)
    kernel\       - JIT-assembled Forth primitives (loaded at boot)
    lib\          - Forth standard library (core.f)
    demos\        - sample programs reachable via the Demos menu
    docs\         - user guide and reference (shown in-window via Help)

Where things live at runtime
----------------------------
The IDE looks for kernel\ and lib\ next to wf64-ui.exe first.
You can move the whole folder anywhere as long as the layout
above stays intact.
"@
Set-Content -Encoding UTF8 (Join-Path $OutDir 'README.txt') $readme
Write-Host "[release] wrote README.txt"

Write-Host ""
Write-Host "[release] done - $OutDir is ready to ship."

#  3. Optional zip 
if ($Zip) {
    $version = & cargo pkgid --manifest-path (Join-Path $repo 'Cargo.toml') 2>&1 |
        Select-String -Pattern '#(.+)$' |
        ForEach-Object { $_.Matches.Groups[1].Value }
    if (-not $version) { $version = 'dev' }
    $zipPath = Join-Path $repo "release\wf64-$version.zip"
    if (Test-Path $zipPath) { Remove-Item $zipPath }
    Compress-Archive -Path "$OutDir\*" -DestinationPath $zipPath
    $zipSize = [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
    Write-Host "[release] zipped to $zipPath ($zipSize MB)"
}
