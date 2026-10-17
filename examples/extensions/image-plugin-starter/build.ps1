param([switch]$Offline, [switch]$Check, [string]$Python = 'python')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$arguments = @((Join-Path $PSScriptRoot 'build.py'))
if ($Offline) { $arguments += '--offline' }
if ($Check) { $arguments += '--check' }
& $Python @arguments
if ($LASTEXITCODE -ne 0) { throw 'Image Gain build failed.' }
