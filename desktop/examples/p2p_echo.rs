use std::{env, error::Error};

use iroh::{Endpoint, EndpointId, endpoint::presets};

const ALPN: &[u8] = b"/seafriend/p2p/1";

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: p2p_echo listen|connect [endpoint-addr]")?;
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;

    println!("endpoint_id={}", endpoint.id());
    println!("endpoint_addr={:?}", endpoint.addr());

    match mode.as_str() {
        "listen" => {
            let incoming = endpoint.accept().await.ok_or("endpoint closed")?;
            let connection = incoming.await?;
            let (mut send, mut receive) = connection.accept_bi().await?;
            let message = receive.read_to_end(64 * 1024).await?;
            println!("received={}", String::from_utf8_lossy(&message));
            send.write_all(b"pong").await?;
            send.finish()?;
            connection.closed().await;
        }
        "connect" => {
            let address = args.next().ok_or("connect requires endpoint-addr")?;
            let endpoint_id: EndpointId = address.parse()?;
            let connection = endpoint.connect(endpoint_id, ALPN).await?;
            let (mut send, mut receive) = connection.open_bi().await?;
            send.write_all(b"ping").await?;
            send.finish()?;
            let response = receive.read_to_end(64 * 1024).await?;
            println!("received={}", String::from_utf8_lossy(&response));
            connection.close(0u8.into(), b"done");
        }
        _ => return Err("mode must be listen or connect".into()),
    }

    endpoint.close().await;
    Ok(())
}
