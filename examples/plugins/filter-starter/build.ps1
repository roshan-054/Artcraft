$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    $targetDirectory = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    cargo build --locked --release --target wasm32-unknown-unknown --target-dir $targetDirectory
    if ($LASTEXITCODE -ne 0) { throw 'Channel Gain filter build failed.' }
    $artifact = Join-Path $targetDirectory 'wasm32-unknown-unknown/release/photocraft_filter_channel_gain.wasm'
    Copy-Item -LiteralPath $artifact -Destination 'Channel-Gain.wasm' -Force
    Write-Output 'Built Channel-Gain.wasm. Install it through Plugins > Install Plug-in.'
} finally { Pop-Location }
