use std::{
    env,
    error::Error,
    io::{self, Write},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};

#[path = "../src/media.rs"]
mod media;

const ALPN: &[u8] = b"/seafriend/media/1";
const PACKET_COUNT: u64 = 3;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: media_echo listen|connect [endpoint-addr-b64]")?;
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;

    println!("endpoint_id={}", endpoint.id());
    let encoded_addr = URL_SAFE_NO_PAD.encode(postcard::to_stdvec(&endpoint.addr())?);
    println!("endpoint_addr_b64={encoded_addr}");
    io::stdout().flush()?;

    match mode.as_str() {
        "listen" => {
            let incoming = endpoint.accept().await.ok_or("endpoint closed")?;
            let connection = incoming.await?;
            let (mut send, mut receive) = connection.accept_bi().await?;
            let mut sequence_tracker = media::SequenceTracker::default();
            for _ in 0..PACKET_COUNT {
                let mut length = [0u8; media::MEDIA_FRAME_HEADER_BYTES];
                receive.read_exact(&mut length).await?;
                let length = u32::from_le_bytes(length) as usize;
                if length > media::MAX_MEDIA_PACKET_BYTES {
                    return Err("media packet length exceeds limit".into());
                }
                let mut encoded = vec![0u8; length];
                receive.read_exact(&mut encoded).await?;
                let mut framed = (length as u32).to_le_bytes().to_vec();
                framed.append(&mut encoded);
                let packet =
                    media::MediaPacket::decode_framed(&framed).map_err(|error| error.to_owned())?;
                sequence_tracker
                    .accept(&packet)
                    .map_err(|error| error.to_owned())?;
                println!(
                    "received kind={:?} stream={} sequence={} payload={} bytes",
                    packet.kind,
                    packet.stream_id,
                    packet.sequence,
                    packet.payload.len()
                );
            }
            send.write_all(b"media-ok").await?;
            send.finish()?;
            connection.closed().await;
        }
        "connect" => {
            let encoded = args.next().ok_or("connect requires endpoint-addr-b64")?;
            let address: EndpointAddr = postcard::from_bytes(&URL_SAFE_NO_PAD.decode(encoded)?)?;
            let connection = endpoint.connect(address, ALPN).await?;
            let (mut send, mut receive) = connection.open_bi().await?;
            for sequence in 0..PACKET_COUNT {
                let packet = media::MediaPacket::video(
                    1,
                    sequence,
                    SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64,
                    320,
                    180,
                    vec![sequence as u8; 320 * 180 * 4],
                );
                let framed = packet.encode_framed()?;
                send.write_all(&framed).await?;
            }
            send.finish()?;
            let acknowledgement = receive.read_to_end(64).await?;
            if acknowledgement.as_slice() != b"media-ok" {
                return Err("media receiver acknowledgement was invalid".into());
            }
            println!("sent {PACKET_COUNT} media packets");
        }
        _ => return Err("mode must be listen or connect".into()),
    }

    endpoint.close().await;
    Ok(())
}
