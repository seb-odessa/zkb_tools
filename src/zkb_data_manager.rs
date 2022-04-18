use anyhow::anyhow;
use clap::Parser;
use rumqttc::{Client, MqttOptions, QoS};
use rumqttc::Event::Incoming;
use rumqttc::Packet;
use rusqlite::{named_params, Connection, Transaction};

use lib::{CmdEvent, DataEvent, Killmail, IdHash};

use chrono::{NaiveDate, NaiveDateTime};

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
        default_value_t = String::from(lib::CMD_TOPIC),
        help = "MQTT topic for the commands"
    )]
    cmd_topic: String,
    #[clap(
        long,
        default_value_t = String::from(lib::DATA_TOPIC),
        help = "MQTT topic for the data"
    )]
    data_topic: String,

    #[clap(
        short,
        long,
        help = "Path to the database file"
    )]
    database: String,

    #[clap(
        short,
        long,
        default_value_t = String::from("2022-01-01"),
        help = "Enable update mode up to YYYY-MM-DD"
    )]
    update_date: String,
}

fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    let client_name = "zkb_data_manager";
    let options = MqttOptions::new(client_name, &config.host, config.port);

    let (mut client, mut eventloop) = Client::new(options, 100);
    client.subscribe(config.data_topic.clone(), QoS::AtMostOnce)?;

    let up_to_date = NaiveDate::parse_from_str(&config.update_date, "%Y-%m-%d")?.and_hms(0,0,0);

    let next: Vec<u8> = bincode::serialize(&CmdEvent::RequestLastHashes(8))?;
    client.publish(config.cmd_topic.clone(), QoS::AtLeastOnce, false, next.clone())?;

    let rt = tokio::runtime::Runtime::new()?;
    let mut conn = create_connection(&config.database)?;
    for (_, event) in eventloop.iter().enumerate() {
        // println!("{:?}", event);
        match event {
            Ok(Incoming(Packet::Publish(event))) => {
                let cmd: DataEvent = bincode::deserialize(event.payload.as_ref())?;
                match cmd {
                    DataEvent::HashesToHandle(hashes) => {
                        println!("Received hashes to porcess {}", hashes.len());
                        let killmails = rt.block_on(async_pre_fetch_killmails(hashes))?;
                        println!("Received killmails to process {}", killmails.len());
                        if acceptable(&killmails, &up_to_date) {
                            let transaction = conn.transaction()?;
                            let ids = fetch_and_insert(killmails, &transaction)?;
                            transaction.commit().map_err(|e| anyhow!(format!("{}", e)))?;

                            let upd: Vec<u8> = bincode::serialize(&CmdEvent::MarkComplete(ids))?;
                            client.publish(config.cmd_topic.clone(), QoS::AtLeastOnce, false, upd.clone())?;
                            client.publish(config.cmd_topic.clone(), QoS::AtLeastOnce, false, next.clone())?;
                        }
                    },
                    DataEvent::KillmailToStore(killmail) => {
                        println!("Received killmail to porcess {} - {}", killmail.killmail_id, killmail.killmail_time);
                        if let Some(ref zkb) = killmail.zkb {
                            let id_hash = (killmail.killmail_id, zkb.hash.clone());
                            let killmails = vec![killmail];
                            let transaction = conn.transaction()?;
                            let _ = fetch_and_insert(killmails, &transaction)?;
                            transaction.commit().map_err(|e| anyhow!(format!("{}", e)))?;

                            let upd: Vec<u8> = bincode::serialize(&CmdEvent::SaveHandledHash(id_hash))?;
                            client.publish(config.cmd_topic.clone(), QoS::AtLeastOnce, false, upd.clone())?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn acceptable(killmails: &Vec<Killmail>, up_to_date: &NaiveDateTime)->bool {
    let format = "%Y-%m-%dT%H:%M:%SZ";
    for killmail in killmails {
        if let Some(date) = NaiveDateTime::parse_from_str(&killmail.killmail_time, format).ok() {
            if up_to_date.timestamp() < date.timestamp() {
                println!("{:?} < {:?}", up_to_date, date);
                return true;
            }
        }
    }
    return false;
}

async fn async_pre_fetch_killmails(hashes: Vec<IdHash>) -> anyhow::Result<Vec<Killmail>> {
    let mut tasks = Vec::new();
    for (id, hash) in hashes {
        let task = tokio::task::spawn(async_fetch_killmail(id, hash.clone()));
        tasks.push(task);
    }

    let mut killmails = Vec::new();
    for task in tasks {
        let killmail = task.await??;
        killmails.push(killmail);
    }

    Ok(killmails)
}

fn fetch_and_insert(killmails: Vec<Killmail>, transaction: &Transaction)-> anyhow::Result<Vec<i32>> {
    const INSERT_KILLMAIL: &str = r"INSERT OR IGNORE INTO killmails VALUES (
        :killmail_id,
        :killmail_time,
        :solar_system_id)";

    const INSERT_PARTICIPANT: &str = r"INSERT OR IGNORE INTO participants VALUES (
        :killmail_id,
        :character_id,
        :corporation_id,
        :alliance_id,
        :ship_type_id,
        :damage,
        :is_victim)";

    let mut insert_killmail_stmt = transaction.prepare(INSERT_KILLMAIL)?;
    let mut insert_participant_stmt = transaction.prepare(INSERT_PARTICIPANT)?;

    let mut ids = Vec::new();
    for killmail in killmails {
        let id = killmail.killmail_id;

        insert_killmail_stmt.execute(named_params! {
            ":killmail_id": killmail.killmail_id,
            ":killmail_time": killmail.killmail_time,
            ":solar_system_id": killmail.solar_system_id
        })?;

        let victim = killmail.victim;
        insert_participant_stmt.execute(named_params!{
            ":killmail_id": killmail.killmail_id,
            ":character_id": victim.character_id,
            ":corporation_id": victim.corporation_id,
            ":alliance_id": victim.alliance_id,
            ":ship_type_id": victim.ship_type_id,
            ":damage": victim.damage_taken,
            ":is_victim": 1
        })?;

        for attacker in killmail.attackers {
            insert_participant_stmt.execute(named_params!{
                ":killmail_id": killmail.killmail_id,
                ":character_id": attacker.character_id,
                ":corporation_id": attacker.corporation_id,
                ":alliance_id": attacker.alliance_id,
                ":ship_type_id": attacker.ship_type_id,
                ":damage": attacker.damage_done,
                ":is_victim": 0
            })?;
        }

        ids.push(id);
    }

    Ok(ids)
}


fn create_connection(url: &String) -> anyhow::Result<Connection> {
    let conn = Connection::open(url)?;
    let _ = conn.execute_batch("
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS killmails(
            killmail_id INTEGER NOT NULL PRIMARY KEY,
            killmail_time TEXT NOT NULL,
            solar_system_id INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS killmail_time_idx ON killmails(killmail_time);

        CREATE TABLE IF NOT EXISTS participants(
            killmail_id INTEGER NOT NULL,
            character_id INTEGER,
            corporation_id INTEGER,
            alliance_id INTEGER,
            ship_type_id INTEGER,
            damage INTEGER NOT NULL,
            is_victim INTEGER NOT NULL,
            UNIQUE(killmail_id, character_id, is_victim),
            FOREIGN KEY(killmail_id) REFERENCES killmails(killmail_id)
        );
        CREATE INDEX IF NOT EXISTS participant_idx ON participants(character_id, corporation_id, alliance_id);
    ").map_err(|e| anyhow!(e))?;

    return Ok(conn);
}

async fn async_fetch_killmail(id: i32, hash: String) -> anyhow::Result<Killmail> {
    let url = format!("https://esi.evetech.net/latest/killmails/{}/{}/", id, hash);
    let mut response = reqwest::get(&url).await?;
    let mut timeout = std::time::Duration::from_secs(3);
    while !response.status().is_success() {
        println!("{} - {}. Retry after {} secs"
            , id
            , response.text().await.unwrap_or_default()
            , timeout.as_secs());
        std::thread::sleep(timeout);
        if timeout.as_secs() < 300 {
            timeout *= 2;
        }
        response = reqwest::get(&url).await?;
    }
    let text = response.text().await?;
    let maybe_killmail = serde_json::from_str::<Killmail>(&text);
    return maybe_killmail.map_err(|e| anyhow!(format!("{}\n{}", e, text)))
}

