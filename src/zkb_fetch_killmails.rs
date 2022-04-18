use anyhow::anyhow;
use clap::Parser;
use futures::future::join_all;
use futures::future::TryFutureExt;
use hyper::body::Buf;
use hyper::Client;
use hyper_tls::HttpsConnector;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use time::{format_description, Date};
use tokio::task;

use std::collections::HashMap;
use std::collections::VecDeque;
use std::convert::TryFrom;
use std::time::Duration;

use lib::{CmdEvent, DailyReport, IdHashBinary};

#[derive(Parser, Debug, Clone)]
#[clap(about = "The tool for receivind killmails from the zkb api", version, author)]
struct Config {
    #[clap(short, long, help = "YYYY-MM-DD")]
    first: String,
    #[clap(short, long, help = "YYYY-MM-DD")]
    last: String,
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
    let ifmt = format_description::parse("[year]-[month]-[day]")?;
    let ofmt = format_description::parse("[year][month][day]")?;

    let config = Config::parse();
    let mut tasks = VecDeque::new();
    let mut current = Date::parse(&config.first, &ifmt)?;
    let last = Date::parse(&config.last, &ifmt)?;
    while current <= last {
        let day = current.format(&ifmt)?;
        let date = current.format(&ofmt)?;
        let future = fetch_map(date).and_then(|map| handle(day, config.clone(), map));
        tasks.push_back(future);
        current = current
            .next_day()
            .ok_or(anyhow!(format!("No next date after {}", current)))?;
    }

    while !tasks.is_empty() {
        let max_count = 3;
        let mut pool = Vec::new();

        while let Some(task) = tasks.pop_front() {
            pool.push(task);
            if pool.len() >= max_count {
                break;
            }
        }

        for result in join_all(pool).await {
            result
                .or_else::<(), _>(|e| {
                    println!("Future: {}", e);
                    Ok(())
                })
                .unwrap();
        }
    }

    Ok(())
}

async fn handle(day: String, cfg: Config, map: HashMap<i32, String>) -> anyhow::Result<()> {
    let client_name = format!("zkb_killmail_receiver_{}", day);
    let mut options = MqttOptions::new(client_name, &cfg.host, cfg.port);
    options.set_keep_alive(Duration::new(5, 0));
    let (client, mut eventloop) = AsyncClient::new(options.clone(), 100);

    task::spawn(async move {
        let topic = cfg.cmd_topic.clone();
        let res = build_report(day.clone(), map)
            .and_then(|report| send(&client, &topic, report))
            .await;
        match res {
            Ok((count, len)) => println!(
                "Sent {} killmails for {}. The message length: {}",
                count, day, len
            ),
            Err(e) => println!("Error occured {}", e),
        }
        client.disconnect().await
    });

    while let Some(_) = eventloop.poll().await.ok() {}

    Ok(())
}

async fn build_report(day: String, map: HashMap<i32, String>) -> anyhow::Result<DailyReport> {
    let mut report = DailyReport::new(day);
    for (id, hash) in &map {
        let id_hash = IdHashBinary::try_from((id, hash)).map_err(|err| anyhow!(err))?;
        report.killmails.push(id_hash);
    }
    return Ok(report);
}

async fn send(
    client: &AsyncClient,
    topic: &String,
    report: DailyReport,
) -> anyhow::Result<(usize, usize)> {
    let date = report.date.clone();
    let count = report.killmails.len();

    let cmd = CmdEvent::SaveDailyReport(report);
    let encoded: Vec<u8> = bincode::serialize(&cmd)?;
    let len = encoded.len();

    let mut res = client
        .publish(topic, QoS::AtLeastOnce, false, encoded.as_slice())
        .await;
    if res.is_err() {
        tokio::time::sleep(Duration::from_millis(300)).await;
        res = client
            .publish(topic, QoS::AtLeastOnce, false, encoded)
            .await;
    }

    res.and_then(|()| Ok((count, len)))
        .map_err(|e| anyhow!(format!("{} for {}", e, date)))
}

async fn fetch_map(day: String) -> anyhow::Result<HashMap<i32, String>> {
    let url = format!("https://zkillboard.com/api/history/{}.json", day);
    let uri = url.parse()?;
    let https = HttpsConnector::new();
    let client = Client::builder().build::<_, hyper::Body>(https);
    let result = client.get(uri).await?;
    let body = hyper::body::aggregate(result).await?;
    let map: HashMap<i32, String> = serde_json::from_reader(body.reader())?;
    Ok(map)
}
