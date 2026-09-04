$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepositoryRoot

cargo test --locked --manifest-path (Join-Path $RepositoryRoot "harness\Cargo.toml")
cargo build --locked --release

$DistDirectory = Join-Path $RepositoryRoot "dist"
New-Item -ItemType Directory -Force -Path $DistDirectory | Out-Null
Copy-Item -Force (Join-Path $RepositoryRoot "target\release\araseo.exe") (Join-Path $DistDirectory "araseo.exe")

Write-Host "Built: $DistDirectory\araseo.exe"
Write-Host "Set ARASEO_EXE in WSL to the /mnt/c/... path of this executable."
