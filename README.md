### 1. Installing dependencies 
```sh
# apt installable
sudo apt update && sudo apt upgrade -y && sudo apt install -y llvm-14-dev liblld-14-dev software-properties-common gcc g++ asciinema containerd cmake zlib1g-dev build-essential python3 python3-dev python3-pip git clang bc jq

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
sudo k3s ctr image import --all-platforms target/wasm32-wasip1/release/img-oci.tar
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
curl -LO https://huggingface.co/second-state/Llama-3.2-3B-Instruct-Uncensored-GGUF/resolve/main/Llama-3.2-3B-Instruct-Uncensored-Q5_K_M.gguf

```

### 5. Apply the kubernetes configuration yaml's

```sh

kubectl apply -f yaml/default-services.yaml
kubectl apply -f yaml/load-balancer.yaml
```

### 5. Query the llama-api-server
#### 1. Sequential test
```sh
sudo k3s kubectl port-forward svc/load-balancer-service 8080:8080 &
PORT_FORWARD_PID=$!

# send some empty requests to save resources and time
for i in {1..10}; do
    echo "=== Request $i ==="
    curl --max-time 60 -X POST http://localhost:8080/v1/chat/completions \
        -H 'Content-Type: application/json' \
        -d "{\"messages\": [{\"role\": \"user\", \"content\": \"\"}], \"model\": \"llama-3-1b\"}" \
        --silent --show-error
    echo -e "\n---\n"
    sleep 0.5
done
# o/p :
# === Request 1 ===
# {"id":"chatcmpl-8221efcf-049d-4eb8-8f99-3398a2768958","object":"chat.completion","created":1753813105,"model":"llama-3-1b-low","choices":[{"index":0,"message":{"content":"I'm ready to help.","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":8,"total_tokens":50}}
# ---

# === Request 2 ===
# {"id":"chatcmpl-f21d6c94-5df0-4602-a9ab-336d058d30c1","object":"chat.completion","created":1753813110,"model":"llama-3-1b-low","choices":[{"index":0,"message":{"content":"I'm sorry, I can't respond to that.","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":13,"total_tokens":55}}
# ---

# === Request 3 ===
# {"id":"chatcmpl-043ecc73-d212-4219-90b7-7e7b9c1d57d3","object":"chat.completion","created":1753813115,"model":"llama-3-1b-low","choices":[{"index":0,"message":{"content":"I can't respond 3.","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":9,"total_tokens":51}}
# ---

# === Request 4 ===
# {"id":"chatcmpl-ba301e5a-ae67-4d55-8fc1-dcc03397258b","object":"chat.completion","created":1753813120,"model":"llama-3-3b-high","choices":[{"index":0,"message":{"content":"I can't fulfill that request.","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":9,"total_tokens":51}}
# ---

# === Request 5 ===
# {"id":"chatcmpl-bff2c2c6-007e-438e-8201-7ecaea5b6187","object":"chat.completion","created":1753813128,"model":"llama-3-1b-low","choices":[{"index":0,"message":{"content":"I cannot provide information or guidance on illegal or harmful activities, especially those that involve non-consensual or exploitative behavior towards children. Is there anything else I can help you with?","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":38,"total_tokens":80}}
# ---

# === Request 6 ===
# {"id":"chatcmpl-10c2a134-a8bf-4344-a771-2cc7a968f747","object":"chat.completion","created":1753813137,"model":"llama-3-1b-low","choices":[{"index":0,"message":{"content":"I cannot provide information or guidance on illegal or harmful activities, especially those that involve non-consensual or exploitative behavior towards children. Is there anything else I can help you with?","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":38,"total_tokens":80}}
# ---

# === Request 7 ===
# {"id":"chatcmpl-ef128fed-5c77-4c9c-9439-7a8eee9d73bc","object":"chat.completion","created":1753813145,"model":"llama-3-1b-low","choices":[{"index":0,"message":{"content":"I cannot provide information or guidance on illegal or harmful activities, especially those that involve non-consensual or exploitative behavior towards children. Can I help you with something else?","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":36,"total_tokens":78}}
# ---

# === Request 8 ===
# {"id":"chatcmpl-3c01a6ec-ae57-425d-85e6-789a92e08990","object":"chat.completion","created":1753813149,"model":"llama-3-3b-high","choices":[{"index":0,"message":{"content":"I can't fulfill that request.","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":9,"total_tokens":51}}
# ---

# === Request 9 ===
# {"id":"chatcmpl-09249335-3a6f-44fc-901b-f0cf280e93c4","object":"chat.completion","created":1753813157,"model":"llama-3-3b-high","choices":[{"index":0,"message":{"content":"I cannot provide a response that includes information or guidance on illegal or harmful activities, especially those that involve children. Is there anything else I can help you with?","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":34,"total_tokens":76}}
# ---

# === Request 10 ===
# {"id":"chatcmpl-20de3377-f344-4f2e-bb2d-e5b4459693a0","object":"chat.completion","created":1753813163,"model":"llama-3-1b-low","choices":[{"index":0,"message":{"content":"I'm here to help. What's on your mind?","role":"assistant"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":42,"completion_tokens":14,"total_tokens":56}}
# ---

# check load balancer logs - to see that the load balancing works
LB_POD=$(sudo k3s kubectl get pods -l app=load-balancer -o jsonpath='{.items[0].metadata.name}')
sudo k3s kubectl logs -f $LB_POD
# Services configured: [Service { name: "llama-low-cost-service", weight: 3 }, Service { name: "llama-high-cost-service", weight: 1 }]
# Selected service: llama-low-cost-service
# Connecting to: 10.43.14.226:8080
# Selected service: llama-low-cost-service
# Connecting to: 10.43.14.226:8080
# Selected service: llama-low-cost-service
# Connecting to: 10.43.14.226:8080
# Selected service: llama-high-cost-service
# Connecting to: 10.43.136.132:8080
# Selected service: llama-low-cost-service
# Connecting to: 10.43.14.226:8080
# Selected service: llama-low-cost-service
# Connecting to: 10.43.14.226:8080
# Selected service: llama-low-cost-service
# Connecting to: 10.43.14.226:8080
# Selected service: llama-high-cost-service
# Connecting to: 10.43.136.132:8080
# Selected service: llama-high-cost-service
# Connecting to: 10.43.136.132:8080
# Selected service: llama-low-cost-service
# Connecting to: 10.43.14.226:8080

kill $PORT_FORWARD_PID

```

#### 1. Concurrency test
```sh
cd
TEST_DIR="load_test_small_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$TEST_DIR"

sudo k3s kubectl port-forward svc/load-balancer-service 8080:8080 &
PORT_FORWARD_PID=$!


for i in {1..10}; do
    (
        request_start=$(date +%s.%3N)
        echo "Request $i started at $(date +%H:%M:%S.%3N)" > "$TEST_DIR/request_${i}_log.txt"
        
        response=$(curl --max-time 300 -w "\nHTTP_CODE:%{http_code}\nTIME_TOTAL:%{time_total}\n" \
            -X POST http://localhost:8080/v1/chat/completions \
            -H 'Content-Type: application/json' \
            -d "{\"messages\": [{\"role\": \"user\", \"content\": \"Short answer: Tell me a fun fact\"}], \"model\": \"llama-3-1b\"}" \
            --silent --show-error 2>"$TEST_DIR/request_${i}_error.log")
        
        request_end=$(date +%s.%3N)
        duration=$(echo "$request_end - $request_start" | bc -l)
        
        echo "$response" | head -n -2 > "$TEST_DIR/response_${i}.json"
        
        echo "Request $i completed at $(date +%H:%M:%S.%3N)" >> "$TEST_DIR/request_${i}_log.txt"
        echo "Duration: ${duration}s" >> "$TEST_DIR/request_${i}_log.txt"
        echo "$response" | tail -n 2 >> "$TEST_DIR/request_${i}_log.txt"
    ) &
done

# check responses/logs
cd && cd $TEST_DIR

kill $PORT_FORWARD_PID
```

### 3. ( _WIP :_ ) API on load-balancer for managing services handling load

To register a new service
```sh
# 1 
kubectl apply -f test_service.yaml

# 2 ( Register the service - working on eliminating this step )
curl -X POST http://localhost:8080/api/register \
-H "Content-Type: application/json" \
-d '{"name": "llama-test-service", "weight": 3}'

# 3 ( restart the load-balancer deployment - working on eliminating this step )
kubectl rollout restart deployment/load-balancer
```

To un-register it
```sh
curl -X DELETE http://localhost:8080/api/unregister/llama-test-service
```

To list the successfully registered services - ie. those able to handle load
```sh
curl http://localhost:8080/api/services
```
