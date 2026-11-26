param(
    [string]$Compiler
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
if (-not $Compiler) {
    $Compiler = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
    if (-not $Compiler) {
        $Compiler = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Inno Setup 6\ISCC.exe'
    }
}
if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw 'Inno Setup 6 is required. Pass the path to ISCC.exe with -Compiler.'
}

Push-Location -LiteralPath $repository
try {
    $metadata = cargo metadata --no-deps --locked --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read the Cargo package metadata.' }
    $version = ($metadata | ConvertFrom-Json).packages.version
    cargo build --release --locked --bin SketioViewer
    if ($LASTEXITCODE -ne 0) { throw 'Sketio Viewer did not build.' }

    $executable = Join-Path $repository 'target\release\SketioViewer.exe'
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable)
    if ($info.ProductVersion -ne $version -or $info.FileDescription -ne 'Sketio Viewer') {
        throw 'The embedded Windows metadata does not match the release.'
    }

    & $Compiler "/DAppVersion=$version" (Join-Path $PSScriptRoot 'SketioViewer.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed.' }

    Join-Path $repository "target\installer\Sketio-Viewer-Setup-$version-x64.exe"
} finally {
    Pop-Location
}
