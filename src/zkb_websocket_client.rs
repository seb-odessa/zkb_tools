use clap::Parser;
use websockets::{WebSocket, Frame, WebSocketError};

use rumqttc::{AsyncClient, MqttOptions, QoS, EventLoop};
use chrono::{DateTime, Utc};

use lib::{Killmail, DataEvent};

#[derive(Parser, Debug, Clone)]
#[clap(about, version, author)]
struct Config {
    #[clap(
        long,
        default_value_t = String::from("localhost"),
        help = "The host name of the MQTT server"
    )]
    host: String,
    #[clap(
        long,
        default_value_t = 1883,
        help = "The port of the MQTT server"
    )]
    port: u16,
    #[clap(
        long,
        default_value_t = String::from(lib::DATA_TOPIC),
        help = "MQTT topic for the data"
    )]
    data_topic: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    let client_name = "zkb_websocket_client";
    let options = MqttOptions::new(client_name, &config.host, config.port);

    let mut ws = WebSocket::connect("wss://zkillboard.com/websocket/").await?;
    ws.send_text(r#"{"action":"sub","channel":"killstream"}"#.to_string()).await?;

    let (client, eventloop) = AsyncClient::new(options, 100);
    let topic = &config.data_topic;
    let _task = tokio::task::spawn(event_loop(eventloop));

    loop {
        let maybe_response = ws.receive().await;
        match maybe_response {
            // println!("{:?}", response);
            Ok(response) => {
                if let Frame::Text{payload, continuation, fin} = response {
                    if !continuation && fin {
                        let killmail = serde_json::from_str::<Killmail>(&payload)?;
                        let id = killmail.killmail_id;
                        let cmd = DataEvent::KillmailToStore(killmail);
                        let encoded: Vec<u8> = bincode::serialize(&cmd)?;
                        client.publish(topic, QoS::AtLeastOnce, false, encoded).await?;
                        let now: DateTime<Utc> = Utc::now();
                        println!("published {} - {}", id, now.format("%a %b %e %T"));
                    }
                }
            }
            Err(e) => {
                println!("Error: {:?}", e);
                if let WebSocketError::ReadError(_) = e {
                    ws.close(None).await?;
                    ws = WebSocket::connect("wss://zkillboard.com/websocket/").await?;
                    ws.send_text(r#"{"action":"sub","channel":"killstream"}"#.to_string()).await?;
                }
            }
        }
    }
    // ws.close(None).await?;
    // client.disconnect().await?;
    // task.await?;

    // Ok(())
}

async fn event_loop(mut eventloop: EventLoop) {
    while let Some(_) = eventloop.poll().await.ok() {}
}