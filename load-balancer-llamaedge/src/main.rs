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
    &services[0].name // fallback - if the weights are not specified, empty list etc - should be validated first :D
}

async fn read_http_request(
    stream: &mut TcpStream,
) -> Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
    // client's address - peer address - here for logging graceful connection closing from client side
    let peer_addr = stream
        .peer_addr()
        .map_or_else(|_| "unknown".to_string(), |addr| addr.to_string());
    println!("handling connection from peer address : {}", peer_addr);

    // to store (as bytes) the input to be echoed
    // let mut buffer = [0; 1024];
    let mut buffer = Vec::new();
    let mut temp_buf = [0; 1024];

    loop {
        let number_of_read_bytes = stream.read(&mut temp_buf).await?;
        // sanity check
        if number_of_read_bytes == 0 {
            println!(
                "client {} closed the connection gracefully - zero bytes read : EOF",
                peer_addr
            );
            break;
        }

        buffer.extend_from_slice(&temp_buf[..number_of_read_bytes]);
        // break loop - if found end of header
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    // start -- separate headers and body from http request
    let request_str = String::from_utf8_lossy(&buffer);
    let (headers, _) = request_str
        .split_once("\r\n\r\n")
        .ok_or("Invalid request")?;
    let headers = headers.to_string();
    let body_start = headers.len() + 4;
    let body = &buffer[body_start..];
    // end -- separate headers and body from http request

    let content_length = headers
        .lines()
        .find(|line| line.to_lowercase().starts_with("content-length:"))
        .and_then(|line| line.split(':').nth(1))
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);

    let mut full_body = body.to_vec();
    while full_body.len() < content_length {
        let number_of_read_bytes = stream.read(&mut temp_buf).await?;
        // sanity check
        if number_of_read_bytes == 0 {
            println!(
                "client {} closed the connection gracefully - zero bytes read : EOF",
                peer_addr
            );
            break;
        }
        full_body.extend_from_slice(&temp_buf[..number_of_read_bytes]);
    }

    Ok((headers, full_body))
}

async fn handle_client(
    mut stream: TcpStream,
    services: Arc<Vec<Service>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // client's address - peer address - here for logging purposes only
    let peer_addr = stream
        .peer_addr()
        .map_or_else(|_| "unknown".to_string(), |addr| addr.to_string());
    println!("handling connection from peer address : {}", peer_addr);

    // read the http request to a tuple
    let (headers, body) = read_http_request(&mut stream).await?;

    // validate the request :
    //  - Reject if path is not /v1/chat/completions
    //  - Reject if not a POST request.
    let request_line = headers.lines().next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "POST" || parts[1] != "/v1/chat/completions" {
        let response = "HTTP/1.1 404 Not Found\r\n\r\n";
        stream.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    // select the service as per 3:1 load-balancing
    let service_name = select_service(&services);
    println!(
        "Selected service {} to serve client {}",
        service_name, peer_addr
    );

    let address = match service_name {
        "llama-low-cost-service" => {
            let host = env::var("LLAMA_LOW_COST_SERVICE_SERVICE_HOST")
                .expect("LLAMA_LOW_COST_SERVICE_SERVICE_HOST not set");
            let port =
                env::var("LLAMA_LOW_COST_SERVICE_SERVICE_PORT").unwrap_or("8080".to_string());
            // concatenate the host and port
            format!("{}:{}", host, port)
        }
        "llama-high-cost-service" => {
            let host = env::var("LLAMA_HIGH_COST_SERVICE_SERVICE_HOST")
                .expect("LLAMA_HIGH_COST_SERVICE_SERVICE_HOST not set");
            let port =
                env::var("LLAMA_HIGH_COST_SERVICE_SERVICE_PORT").unwrap_or("8080".to_string());
            // concatenate({}:{}) the host and port
            format!("{}:{}", host, port)
        }
        _ => {
            return Err(format!("Unknown service: {}\nPlease register it first\ndynamic service/pod reginstration coming soon", service_name).into());
        }
    };

    println!("Service selected,\nnow connecting to it at: {}", address);

    let mut backend_stream = TcpStream::connect(&address).await?;

    backend_stream.write_all(headers.as_bytes()).await?; // writes the original headers to backend
    backend_stream.write_all(b"\r\n\r\n").await?; // add \r\n\r\n to terminate header explicitly
    backend_stream.write_all(&body).await?; // writes the original body to backend

    //  stream backend response to client
    tokio::io::copy(&mut backend_stream, &mut stream).await?;

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // start -- recognize the servers exposed out there using their own services
    let services_str = env::var("SERVICES").expect(
        "maybe the SERVICES env var is not set \n
        (e.g., 'llama-low-cost-service,3;llama-high-cost-service,1')",
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

    println!(
        "Services that are currently being seen as environment variables :\n{:?}",
        services
    );
    // end -- recognize the servers exposed out there using their own services

    // start -- tcplistener, listening loop
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());

    let listener = TcpListener::bind(&addr)
        .await
        .expect("Failed to bind to address");
    println!("Server listening on address : {}", addr);

    // loop to keep listening to new connections on the tcplistener address
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
    // end -- tcplistener, listening loop
}
