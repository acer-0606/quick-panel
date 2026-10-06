param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [Parameter(Mandatory=$true)][string]$Node,
    [Parameter(Mandatory=$true)][string]$Pnpm,
    [Parameter(Mandatory=$true)][string]$NodeAssets
)
$ErrorActionPreference = "Stop"
Push-Location (Join-Path $PSScriptRoot "..")
try {
    if ((& $Node --version) -ne "v24.19.0") { throw "Node 24.19.0 required" }
    $env:PATH = (Split-Path $Node) + ";" + $env:PATH
    & $Node $Pnpm --dir agent-runtime install --ignore-scripts --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "Frozen dependency install failed" }
    & $Node agent-runtime/scripts/build.mjs
    if ($LASTEXITCODE -ne 0) { throw "Pi bundle build failed" }
    $env:CARGO_INCREMENTAL = "0"
    $env:CARGO_BUILD_JOBS = "1"
    $env:CARGO_PROFILE_DEV_DEBUG = "0"
    $env:CARGO_PROFILE_TEST_DEBUG = "0"
    cargo build --release --locked --target $Target
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    python scripts/package-portable.py --target $Target --node-assets $NodeAssets
    if ($LASTEXITCODE -ne 0) { throw "Package verification failed" }
} finally { Pop-Location }
