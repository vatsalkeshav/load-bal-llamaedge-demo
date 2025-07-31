use rand::Rng;
use std::env;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
struct Service {
    name: String,
    weight: u32,
}

fn select_service(services: &[Service]) -> &str {
    let total_weight: u32 = services.iter().map(|s| s.weight).sum();
    let mut rng = rand::rng();
    let mut choice = rng.random_range(0..total_weight);
    for service in services {
        if choice < service.weight {
            return &service.name;
        }
        choice -= service.weight;
    }
    &services[0].name
}

async fn read_http_request(
    stream: &mut TcpStream,
) -> Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
    let mut buffer = Vec::new();
    let mut temp_buf = [0; 1024];

    loop {
        let bytes_read = stream.read(&mut temp_buf).await?;
        if bytes_read == 0 {
            return Err("Connection closed".into());
        }
        buffer.extend_from_slice(&temp_buf[..bytes_read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    let request_str = String::from_utf8_lossy(&buffer);
    let (headers, _) = request_str
        .split_once("\r\n\r\n")
        .ok_or("Invalid request")?;
    let headers = headers.to_string();
    let body_start = headers.len() + 4;
    let body = &buffer[body_start..];

    let content_length = headers
        .lines()
        .find(|line| line.to_lowercase().starts_with("content-length:"))
        .and_then(|line| line.split(':').nth(1))
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);

    let mut full_body = body.to_vec();
    while full_body.len() < content_length {
        let bytes_read = stream.read(&mut temp_buf).await?;
        if bytes_read == 0 {
            return Err("Connection closed before receiving full body".into());
        }
        full_body.extend_from_slice(&temp_buf[..bytes_read]);
    }

    Ok((headers, full_body))
}

async fn handle_client(
    mut stream: TcpStream,
    services: Arc<Vec<Service>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (headers, body) = read_http_request(&mut stream).await?;

    let request_line = headers.lines().next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "POST" || parts[1] != "/v1/chat/completions" {
        let response = "HTTP/1.1 404 Not Found\r\n\r\n";
        stream.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    let service_name = select_service(&services);
    println!("Selected service: {}", service_name);

    let address = match service_name {
        "llama-low-cost-service" => {
            let host = env::var("LLAMA_LOW_COST_SERVICE_SERVICE_HOST")
                .expect("LLAMA_LOW_COST_SERVICE_SERVICE_HOST not set");
            let port =
                env::var("LLAMA_LOW_COST_SERVICE_SERVICE_PORT").unwrap_or("8080".to_string());
            format!("{}:{}", host, port)
        }
        "llama-high-cost-service" => {
            let host = env::var("LLAMA_HIGH_COST_SERVICE_SERVICE_HOST")
                .expect("LLAMA_HIGH_COST_SERVICE_SERVICE_HOST not set");
            let port =
                env::var("LLAMA_HIGH_COST_SERVICE_SERVICE_PORT").unwrap_or("8080".to_string());
            format!("{}:{}", host, port)
        }
        _ => {
            return Err(format!("Unknown service: {}", service_name).into());
        }
    };

    println!("Connecting to: {}", address);

    let mut backend_stream = TcpStream::connect(&address).await?;

    backend_stream.write_all(headers.as_bytes()).await?;
    backend_stream.write_all(b"\r\n\r\n").await?;
    backend_stream.write_all(&body).await?;

    tokio::io::copy(&mut backend_stream, &mut stream).await?;

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let services_str = env::var("SERVICES").expect(
        "maybe SERVICES env var is not set (e.g., 'llama-low-cost-service,3;llama-high-cost-service,1')",
    );

    let services: Vec<Service> = services_str
        .split(';')
        .map(|s| {
            let parts: Vec<&str> = s.split(',').collect();
            Service {
                name: parts[0].to_string(),
                weight: parts[1].parse().expect("Weight must be a number"),
            }
        })
        .collect();

    // `let services = Rc::new(services);` could be used due to wasm's single threaded nature
    // but `Arc` works well with `tokio::spawn`
    let services = Arc::new(services);

    println!("Services configured: {:?}", services);

    let listener = TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("Failed to bind to address");
    println!("Load balancer running on port 8080...");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let services_clone = services.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, services_clone).await {
                        eprintln!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Failed to accept connection: {}", e);
            }
        }
    }
}
