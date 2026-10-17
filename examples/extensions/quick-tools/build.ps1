$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    cargo build --release --target wasm32-unknown-unknown
    if ($LASTEXITCODE -ne 0) { throw 'Extension build failed.' }
    Copy-Item -LiteralPath 'target/wasm32-unknown-unknown/release/photocraft_extension_quick_tools.wasm' -Destination 'quick-tools.pcext' -Force
    Write-Output 'Built quick-tools.pcext — install through Plugins > Manage Plugins.'
} finally { Pop-Location }
