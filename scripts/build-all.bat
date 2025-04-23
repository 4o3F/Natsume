call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"

set CMAKE_GENERATOR=
echo "Building client..."
cargo build --bin natsume_client --features="client"

echo "Building server..."
cargo build --bin natsume_server --features="server"

echo "Builds completed!"
