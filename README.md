
### 1. Installing dependencies 
```sh
# apt installable
sudo apt update && sudo apt upgrade -y && sudo apt install -y llvm-14-dev liblld-14-dev software-properties-common gcc g++ asciinema containerd cmake zlib1g-dev build-essential python3 python3-dev python3-pip git clang

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && source $HOME/.cargo/env
rustup target add wasm32-wasip1
exec $SHELL

# WasmEdge + WASINN plugin
curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash -s -- --plugins wasi_nn-ggml -v 0.14.1 # binaries and plugin in $HOME/.wasmedge
source $HOME/.bashrc

# Runwasi's containerd-shim-wasmedge-v1
cd
git clone https://github.com/containerd/runwasi.git
cd runwasi
./scripts/setup-linux.sh
make build-wasmedge
INSTALL="sudo install" LN="sudo ln -sf" make install-wasmedge
which containerd-shim-wasmedge-v1 # verify

# deps - k3s installation
cd
curl -sfL https://get.k3s.io | sh - 
sudo chmod 777 /etc/rancher/k3s/k3s.yaml # hack
```


### 3. Building image ghcr.io/second-state/llama-api-server:latest
This step builds the `ghcr.io/second-state/llama-api-server:latest` image and imports it to the k3s' containerd's local image store

> same as `Build and import demo image` from [README.md]](https://github.com/second-state/runwasi-wasmedge-demo/README.md)

```sh
# build llama-server-wasm
cd

git clone --recurse-submodules https://github.com/second-state/runwasi-wasmedge-demo.git

cd runwasi-wasmedge-demo

# edit makefile to eliminate containerd version error
sed -i -e '/define CHECK_CONTAINERD_VERSION/,/^endef/{
s/Containerd version must be/WARNING: Containerd version should be/
/exit 1;/d
}' Makefile

# Manually removed the dependency on wasi_logging due to issue #4003.
git -C apps/llamaedge apply $PWD/disable_wasi_logging.patch

OPT_PROFILE=release RUSTFLAGS="--cfg wasmedge --cfg tokio_unstable" make apps/llamaedge/llama-api-server

# place llama-server-img in k3s' containerd local store
cd $HOME/runwasi-wasmedge-demo/apps/llamaedge/llama-api-server
oci-tar-builder --name llama-api-server \
    --repo ghcr.io/second-state \
    --tag latest \
    --module target/wasm32-wasip1/release/llama-api-server.wasm \
    -o target/wasm32-wasip1/release/img-oci.tar # Create OCI image from the WASM binary
sudo k3s ctr image import --all-platforms $HOME/runwasi-wasmedge-demo/apps/llamaedge/llama-api-server/target/wasm32-wasip1/release/img-oci.tar 
sudo k3s ctr images ls # verify the import
```

### 5. Build the load-balancer-app
```sh
cd load-balancer-llamaedge
cargo build --target wasm32-wasip1 --release

oci-tar-builder --name load-balancer-llamaedge \
    --repo ghcr.io/second-state \
    --tag latest \
    --module target/wasm32-wasip1/release/load-balancer-llamaedge.wasm \
    -o target/wasm32-wasip1/release/img-oci.tar # Create OCI image from the WASM binary
sudo k3s ctr image import --all-platforms /target/wasm32-wasip1/release/img-oci.tar
sudo k3s ctr images ls # verify the import
```


### 4. Download the gguf model needed by llama-api-server
```sh 
cd
sudo mkdir -p models
sudo chmod 777 models  # ensure it's readable by k3s
cd models
curl -LO https://huggingface.co/second-state/Llama-3.2-1B-Instruct-GGUF/resolve/main/Llama-3.2-1B-Instruct-Q5_K_M.gguf
curl -LO https://huggingface.co/second-state/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q5_K_M.gguf
```

### 5. Create the kubernetes configuration yaml

```sh
kubectl apply -f deployment.yaml
```

### 5. Query the llama-api-server
```sh
sudo k3s kubectl port-forward svc/load-balancer-service 8080:8080

for i in {1..6}; do
  curl -X POST http://localhost:8080/v1/chat/completions \
    -H 'accept: application/json' \
    -H 'Content-Type: application/json' \
    -d "{\"messages\": [{\"role\": \"system\", \"content\": \"You are a helpful assistant.\"}, {\"role\": \"user\", \"content\": \"Tell me about AI topic $i\"}], \"model\": \"llama-3-1b\"}" \
    > response_$i.json
  echo "Request $i completed, saved to response_$i.json"
  sleep 1
done

for i in {1..6}; do
  cat response_$i.json | jq '.choices[0].message.content'
done
```

### 6. Cleanup
```sh
cd
sudo k3s kubectl delete -f deployment.yaml
```