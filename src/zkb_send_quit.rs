use clap::Parser;
use rumqttc::{AsyncClient, MqttOptions, QoS};

use lib::CmdEvent;

#[derive(Parser, Debug, Clone)]
#[clap(about, version, author)]
struct Config {
    #[clap(
        short,
        long,
        default_value_t = String::from(lib::CMD_TOPIC),
        help = "MQTT topic for the commands"
    )]
    cmd_topic: String,
    #[clap(
        short,
        long,
        default_value_t = String::from("localhost"),
        help = "The host name of the MQTT server"
    )]
    host: String,
    #[clap(
        short,
        long,
        default_value_t = 1883,
        help = "The port of the MQTT server"
    )]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    let client_name = "zkb_send_quit";
    let options = MqttOptions::new(client_name, &config.host, config.port);
    let cmd = CmdEvent::Quit;
    let encoded: Vec<u8> = bincode::serialize(&cmd)?;

    let (client, mut eventloop) = AsyncClient::new(options, 100);
    client.publish(config.cmd_topic, QoS::AtLeastOnce, false, encoded).await?;
    client.disconnect().await?;
    while let Some(_) = eventloop.poll().await.ok() {}
    Ok(())
}
