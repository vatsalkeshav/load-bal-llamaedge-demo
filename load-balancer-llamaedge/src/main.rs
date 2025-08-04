use rand::Rng;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Service {
    name: String,
    weight: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct RegisterRequest {
    name: String,
    weight: u32,
}

#[derive(Debug, Clone)]
struct ServiceRegistry {
    services: Arc<RwLock<Vec<Service>>>,
}

impl ServiceRegistry {
    fn new() -> Self {
        Self {
            services: Arc::new(RwLock::new(Vec::new())),
        }
    }

    async fn register_service(&self, service: Service) {
        println!(
            "Attempting to register service: {} (weight: {})",
            service.name, service.weight
        );

        // Validate that the service has required environment variables
        if get_service_address(&service.name).is_none() {
            println!(
                "Failed to register service '{}' - environment variables not found",
                service.name
            );
            return;
        }

        let mut services = self.services.write().await;
        if let Some(existing) = services.iter_mut().find(|s| s.name == service.name) {
            println!(
                "Found existing service '{}' with weight: {}",
                existing.name, existing.weight
            );
            println!(
                "Updated existing service: {} (weight: {} -> {})",
                service.name, existing.weight, service.weight
            );
            *existing = service;
        } else {
            println!(
                "Registered new service: {} (weight: {})",
                service.name, service.weight
            );
            services.push(service);
        }

        println!("Total services registered: {}", services.len());
    }

    async fn unregister_service(&self, name: &str) -> bool {
        let mut services = self.services.write().await;
        let initial_len = services.len();
        services.retain(|s| s.name != name);
        let removed = services.len() < initial_len;
        if removed {
            println!("Unregistered service: {}", name);
        } else {
            println!("Failed to unregister service (not found): {}", name);
        }
        removed
    }

    async fn list_services(&self) -> Vec<Service> {
        let services = self.services.read().await;
        services.clone()
    }
}

fn get_service_address(service_name: &str) -> Option<String> {
    let env_base = service_name.to_uppercase().replace("-", "_");
    let host_var = format!("{}_SERVICE_HOST", env_base);
    let port_var = format!("{}_SERVICE_PORT", env_base);

    let host = env::var(&host_var).ok()?;
    let port = env::var(&port_var).unwrap_or_else(|_| "8080".to_string());
    let address = format!("{}:{}", host, port);
    println!(
        "Resolved service '{}' to address: {}",
        service_name, address
    );
    Some(address)
}

fn select_service(services: &[Service]) -> Option<&Service> {
    if services.is_empty() {
        println!("No services available for selection");
        return None;
    }

    let total_weight: u32 = services.iter().map(|s| s.weight).sum();
    if total_weight == 0 {
        println!(
            "All services have zero weight, selecting first service: {}",
            services[0].name
        );
        return services.first();
    }

    let mut rng = rand::thread_rng();
    let mut choice = rng.gen_range(0..total_weight);
    let original_choice = choice;

    for service in services {
        if choice < service.weight {
            println!(
                "Selected service '{}' (choice: {}/{}, weight: {})",
                service.name, original_choice, total_weight, service.weight
            );
            return Some(service);
        }
        choice -= service.weight;
    }

    println!("Fallback to first service: {}", services[0].name);
    services.first()
}

async fn read_request(
    stream: &mut TcpStream,
    peer_addr: std::net::SocketAddr,
) -> Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
    let mut buffer = Vec::new();
    let mut temp_buf = [0; 1024];

    loop {
        let bytes_read = stream.read(&mut temp_buf).await?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp_buf[..bytes_read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    let request_str = String::from_utf8_lossy(&buffer);
    let (headers, _) = request_str
        .split_once("\r\n\r\n")
        .unwrap_or((&request_str, ""));
    let body_start = headers.len() + 4;
    let body = if body_start < buffer.len() {
        buffer[body_start..].to_vec()
    } else {
        Vec::new()
    };

    println!(
        "Read request from {} - headers size: {}, body size: {}",
        peer_addr,
        headers.len(),
        body.len()
    );
    Ok((headers.to_string(), body))
}

async fn handle_api_request(
    mut stream: TcpStream,
    registry: Arc<ServiceRegistry>,
    method: &str,
    path: &str,
    body: &[u8],
    peer_addr: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Handling API request from {}: {} {}",
        peer_addr, method, path
    );

    match (method, path) {
        ("POST", "/api/register") => {
            if let Ok(req) = serde_json::from_slice::<RegisterRequest>(body) {
                println!(
                    "Registration request from {} for service: {} (weight: {})",
                    peer_addr, req.name, req.weight
                );
                if get_service_address(&req.name).is_some() {
                    let service = Service {
                        name: req.name,
                        weight: req.weight,
                    };
                    registry.register_service(service).await;
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\n\r\nRegistered")
                        .await?;
                } else {
                    println!(
                        "Failed to find environment variables for service: {} (request from {})",
                        req.name, peer_addr
                    );
                    stream
                        .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\nService env vars not found")
                        .await?;
                }
            } else {
                println!("Invalid JSON in registration request from {}", peer_addr);
                stream
                    .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\nInvalid JSON")
                    .await?;
            }
        }
        ("DELETE", path) if path.starts_with("/api/unregister/") => {
            let service_name = path.strip_prefix("/api/unregister/").unwrap_or("");
            println!(
                "Unregistration request from {} for service: {}",
                peer_addr, service_name
            );
            if registry.unregister_service(service_name).await {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\n\r\nUnregistered")
                    .await?;
            } else {
                stream
                    .write_all(b"HTTP/1.1 404 Not Found\r\n\r\nService not found")
                    .await?;
            }
        }
        ("GET", "/api/services") => {
            let services = registry.list_services().await;
            println!(
                "Listing {} registered services for request from {}",
                services.len(),
                peer_addr
            );
            let json = serde_json::to_string(&services)?;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{}",
                json
            );
            stream.write_all(response.as_bytes()).await?;
        }
        _ => {
            println!(
                "Unknown API request from {}: {} {}",
                peer_addr, method, path
            );
            stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await?;
        }
    }
    Ok(())
}

async fn handle_client(
    mut stream: TcpStream,
    registry: Arc<ServiceRegistry>,
) -> Result<(), Box<dyn std::error::Error>> {
    let peer_addr = stream
        .peer_addr()
        .unwrap_or_else(|_| "unknown".parse().unwrap());
    println!("New connection from: {}", peer_addr);

    let (headers, body) = read_request(&mut stream, peer_addr).await?;

    let request_line = headers.lines().next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();

    if parts.len() != 3 {
        println!("Invalid request line from {}: {}", peer_addr, request_line);
        stream
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }

    let method = parts[0];
    let path = parts[1];
    println!("Request from {}: {} {}", peer_addr, method, path);

    if path.starts_with("/api/") {
        return handle_api_request(stream, registry, method, path, &body, peer_addr).await;
    }

    if method != "POST" || path != "/v1/chat/completions" {
        println!(
            "Unsupported request from {}: {} {}",
            peer_addr, method, path
        );
        stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await?;
        return Ok(());
    }

    let services = registry.list_services().await;
    println!("Available services for load balancing: {}", services.len());

    let selected_service = match select_service(&services) {
        Some(service) => service,
        None => {
            println!("No services available for request from {}", peer_addr);
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n")
                .await?;
            return Ok(());
        }
    };

    let address = match get_service_address(&selected_service.name) {
        Some(addr) => addr,
        None => {
            println!(
                "Failed to resolve address for service: {}",
                selected_service.name
            );
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n")
                .await?;
            return Ok(());
        }
    };

    println!(
        "Forwarding request from {} to service '{}' at {}",
        peer_addr, selected_service.name, address
    );

    match TcpStream::connect(&address).await {
        Ok(mut backend_stream) => {
            backend_stream.write_all(headers.as_bytes()).await?;
            backend_stream.write_all(b"\r\n\r\n").await?;
            backend_stream.write_all(&body).await?;

            let bytes_copied = tokio::io::copy(&mut backend_stream, &mut stream).await?;
            println!(
                "Completed request from {} via '{}' - {} bytes returned",
                peer_addr, selected_service.name, bytes_copied
            );
        }
        Err(e) => {
            println!(
                "Failed to connect to service '{}' at {}: {}",
                selected_service.name, address, e
            );
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n")
                .await?;
        }
    }

    Ok(())
}

async fn initialize_services_from_env(registry: Arc<ServiceRegistry>) {
    println!("Initializing services from environment variables...");

    if let Ok(services_str) = env::var("SERVICES") {
        println!("Found SERVICES env var: {}", services_str);

        for service_def in services_str.split(';') {
            let parts: Vec<&str> = service_def.split(',').collect();
            if parts.len() >= 2 {
                let name = parts[0].trim().to_string();
                if let Ok(weight) = parts[1].trim().parse::<u32>() {
                    if get_service_address(&name).is_some() {
                        let service = Service { name, weight };
                        registry.register_service(service).await;
                    } else {
                        println!(
                            "Skipping service '{}' - environment variables not found",
                            name
                        );
                    }
                } else {
                    println!("Invalid weight for service '{}': {}", name, parts[1]);
                }
            } else {
                println!("Invalid service definition: {}", service_def);
            }
        }
    } else {
        println!("No SERVICES environment variable found");
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    println!("Starting load balancer...");

    let registry = Arc::new(ServiceRegistry::new());
    initialize_services_from_env(registry.clone()).await;

    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");

    println!("Load balancer listening on: {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, peer_addr)) => {
                println!("Accepted connection from: {}", peer_addr);
                let registry_clone = registry.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, registry_clone).await {
                        println!("Error handling client {}: {}", peer_addr, e);
                    }
                });
            }
            Err(e) => {
                println!("Failed to accept connection: {}", e);
            }
        }
    }
}
