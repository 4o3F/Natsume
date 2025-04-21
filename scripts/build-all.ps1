# 构建 client（不带 server feature）
Write-Host "Building client..."
cargo build --bin client --features="client"

# 构建 server（带 server feature）
Write-Host "Building server..."
cargo build --bin server --features="server"

Write-Host "Builds completed!"
