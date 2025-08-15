#!/bin/bash

set -euo pipefail

sudo apt update && sudo apt upgrade -y
sudo apt install -y llvm-14-dev liblld-14-dev software-properties-common \
    gcc g++ asciinema containerd cmake zlib1g-dev build-essential \
    python3 python3-dev python3-pip git clang bc jq

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
rustup target add wasm32-wasip1

curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash -s -- --plugins wasi_nn-ggml -v 0.14.1
export PATH="$HOME/.wasmedge/bin:$PATH"

cd "$HOME"
git clone https://github.com/containerd/runwasi.git
cd runwasi
./scripts/setup-linux.sh
make build-wasmedge
INSTALL="sudo install" LN="sudo ln -sf" make install-wasmedge
which containerd-shim-wasmedge-v1

cd "$HOME"
curl -sfL https://get.k3s.io | sh -
sudo chmod 777 /etc/rancher/k3s/k3s.yaml

echo "=== Cloning llama-api-server demo ==="
cd "$HOME"
git clone --recurse-submodules https://github.com/second-state/runwasi-wasmedge-demo.git
cd runwasi-wasmedge-demo

sed -i -e '/define CHECK_CONTAINERD_VERSION/,/^endef/{
s/Containerd version must be/WARNING: Containerd version should be/
/exit 1;/d
}' Makefile

git -C apps/llamaedge apply "$PWD/disable_wasi_logging.patch"

OPT_PROFILE=release RUSTFLAGS="--cfg wasmedge --cfg tokio_unstable" make apps/llamaedge/llama-api-server

cd "$HOME/runwasi-wasmedge-demo/apps/llamaedge/llama-api-server"
oci-tar-builder --name llama-api-server \
    --repo ghcr.io/second-state \
    --tag latest \
    --module target/wasm32-wasip1/release/llama-api-server.wasm \
    -o target/wasm32-wasip1/release/img-oci.tar

sudo k3s ctr image import --all-platforms target/wasm32-wasip1/release/img-oci.tar
sudo k3s ctr images ls

cd
cp -R /mnt/mac/Users/dev/someplace/models .
chmod -R 777 models

sudo k3s kubectl apply -f load-balancer-llamaedge/yaml/load-balancer.yaml
sudo k3s kubectl apply -f load-balancer-llamaedge/yaml/default-services.yaml
sudo k3s kubectl apply -f watcher/yaml/watcher.yaml
