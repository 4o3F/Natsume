#!/bin/bash

echo "Building client..."
cross build --bin natsume_client --features="client" --target x86_64-unknown-linux-musl

echo "Building server..."
cross build --bin natsume_server --features="server" --target x86_64-unknown-linux-musl

echo "Builds completed!"
