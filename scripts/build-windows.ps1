param([string]$Target = "x86_64-pc-windows-msvc")
$ErrorActionPreference = "Stop"
Push-Location (Join-Path $PSScriptRoot "..")
try {
    cargo build --release --locked --target $Target
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    $bundle = Join-Path (Get-Location) "dist/QuickPanel-Windows-x64"
    New-Item -ItemType Directory -Force $bundle | Out-Null
    Copy-Item "target/$Target/release/quick-panel.exe" "$bundle/QuickPanel.exe"
    Copy-Item "README.md", "LICENSE" $bundle
    Compress-Archive -Path "$bundle/*" -DestinationPath "dist/QuickPanel-Windows-x64.zip" -Force
    Write-Output "Created dist/QuickPanel-Windows-x64.zip"
} finally { Pop-Location }
