param([switch]$Offline)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Push-Location $PSScriptRoot
try {
    $buildArguments = @('build', '--locked', '--release', '--target', 'wasm32-unknown-unknown')
    if ($Offline) { $buildArguments += '--offline' }
    & cargo @buildArguments
    if ($LASTEXITCODE -ne 0) { throw 'Extension build failed.' }

    $targetDirectory = if ([string]::IsNullOrEmpty($env:CARGO_TARGET_DIR)) { 'target' } else { $env:CARGO_TARGET_DIR }
    $module = Join-Path $targetDirectory 'wasm32-unknown-unknown/release/hello_photocraft.wasm'
    Copy-Item -LiteralPath $module -Destination 'Hello-PhotoCraft.pcext' -Force
    Write-Output 'Built Hello-PhotoCraft.pcext. Install it through Plugins > Manage Plugins.'
} finally {
    Pop-Location
}
