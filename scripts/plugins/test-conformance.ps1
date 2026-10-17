param(
    [Parameter(Mandatory = $true)][string]$FreshExport,
    [Parameter(Mandatory = $true)][string]$ToolPath
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$env:PHOTOCRAFT_PLUGIN_CARGO = (Get-Command cargo -ErrorAction Stop).Source
$env:PHOTOCRAFT_PLUGIN_BUILD_PATH = $ToolPath
$env:PHOTOCRAFT_NO_PRINT = '1'
Push-Location -LiteralPath $repository
try {
    & cargo test -p photocraft-plugin --locked --offline
    if ($LASTEXITCODE -ne 0) { throw 'Conformance tool tests failed.' }
    & cargo run -p photocraft-plugin --locked --offline -- build-export (Join-Path $repository 'examples/extensions/image-plugin-starter') $FreshExport
    if ($LASTEXITCODE -ne 0) { throw 'Clean standalone starter build or package validation failed.' }
} finally {
    Pop-Location
}
