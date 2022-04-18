use anyhow::anyhow;
use clap::Parser;
use rumqttc::Event::Incoming;
use rumqttc::Packet;
use rumqttc::{Client, MqttOptions, QoS};
use rusqlite::{params, Connection, Transaction};
use serde::Serialize;

use lib::{CmdEvent, DataEvent, DailyReport, IdHashBinary, IdHash};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

type TSharedQueue = Arc<Mutex<VecDeque<CmdEvent>>>;
type TSharedCond = Arc<(Mutex<bool>, Condvar)>;

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
        help = "MQTT topic for thedata"
    )]
    data_topic: String,

    #[clap(
        short,
        long,
        help = "Path to the database file"
    )]
    database: String,
}

fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    let mut options = MqttOptions::new("zkb_database", config.host.clone(), config.port);
    options
        .set_keep_alive(Duration::new(5, 0))
        .set_max_packet_size(1024 * 1024, 1024 * 1024);

    let queue = Arc::new(Mutex::new(VecDeque::new()));
    let cond = Arc::new((Mutex::new(false), Condvar::new()));

    let (mut client, mut eventloop) = Client::new(options, 100);

    let cloned_cfg = config.clone();
    let cloned_queue = queue.clone();
    let cloned_cond = Arc::clone(&cond);
    let cloned_client = client.clone();
    let pid = thread::spawn(move || worker(cloned_queue, cloned_cond, cloned_client, cloned_cfg));

    client.subscribe(config.cmd_topic, QoS::AtMostOnce)?;

    let mut ready_to_exit = false;
    for (_, event) in eventloop.iter().enumerate() {
        // println!("{:?}", event);
        match event {
            Ok(Incoming(Packet::Publish(event))) => {
                let cmd: CmdEvent = bincode::deserialize(event.payload.as_ref())?;
                ready_to_exit = cmd == CmdEvent::Quit;
                while !enqueue(&queue, &cmd) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                notify(&cond);
            }
            Ok(_) => {}
            Err(e) => println!("{:?}", e),
        }
        if ready_to_exit {
            break;
        }
    }
    let _ = pid.join().expect("thread::spawn failed");
    Ok(())
}

fn wait(cond: &TSharedCond) {
    let (lock, cvar) = &**cond;
    let mut started = lock.lock().unwrap();
    while !*started {
        started = cvar.wait(started).unwrap();
    }
    *started = false;
}

fn notify(cond: &TSharedCond) {
    {
        let (lock, cvar) = &**cond;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();
    }
}

fn enqueue(queue: &TSharedQueue, cmd: &CmdEvent) -> bool {
    let mut lock = queue.try_lock();
    if let Ok(ref mut queue) = lock {
        queue.push_back(cmd.clone());
        return true;
    } else {
        println!("try_lock failed in enqueue");
        return false;
    }
}

fn dequeue(queue: &TSharedQueue) -> Option<CmdEvent> {
    let mut lock = queue.try_lock();
    if let Ok(ref mut queue) = lock {
        return queue.pop_front();
    } else {
        println!("try_lock failed in dequeue");
    }
    return None;
}

fn worker(queue: TSharedQueue, cond: TSharedCond, mut client: Client, cfg: Config) -> anyhow::Result<()> {
    let url = cfg.database.clone();

    let mut conn = Connection::open(url)?;
    let _ = conn
        .execute(
            "CREATE TABLE IF NOT EXISTS hashes(
            id INTEGER PRIMARY KEY NOT NULL,
            hash BLOB NOT NULL,
            state INTEGER NOT NULL DEFAULT 0
        );",
            [],
        )
        .map_err(|e| anyhow!(e))?;

    let mut ready_to_exit = false;
    while !ready_to_exit {
        let data_topic = cfg.data_topic.clone();
        if let Some(cmd) = dequeue(&queue) {
            match cmd {
                CmdEvent::SaveDailyReport(report) => {
                    let date = report.date.clone();
                    let count = insert(report, &mut conn)?;
                    println!("Inserted {} killmails for '{}'", count, date);
                },
                CmdEvent::RequestLastHashes(count) => {
                    let payload = query_hashes(count, &conn)?;
                    let response = DataEvent::HashesToHandle(payload);
                    publish(&mut client, &data_topic, &response)?;
                    println!("Published {} killmails for quering details", count);
                },
                CmdEvent::MarkComplete(ids) => {
                    let updated = mark_complete(&ids, &conn)?;
                    println!("The {}/{} killmail saved: {:?}", updated, ids.len(), ids);
                },
                CmdEvent::SaveHandledHash((id, hash)) => {
                    save_handled_hash(id, hash, &conn)?;
                    println!("The {} killmail inserted as complete", id);
                }
                CmdEvent::Quit => {
                    ready_to_exit = true;
                    println!("Received 'Quit' command. Going to exit");
                },
                _ => {
                    panic!("Unreachable");
                }
            }
        } else {
            wait(&cond);
        }
    }
    Ok(())
}

fn publish<T: Serialize>(client: &mut Client, topic: &String, response: &T) -> anyhow::Result<()> {
    let encoded: Vec<u8> = bincode::serialize(&response)?;
    client.publish(topic, QoS::AtLeastOnce, false, encoded)
        .map_err(|e| anyhow!(format!("{}", e)))
}

fn insert(report: DailyReport, conn: &mut Connection) -> anyhow::Result<usize> {
    let transaction = conn.transaction()?;
    let count = insert_impl(report, &transaction)?;
    transaction
        .commit()
        .map(|()|{count})
        .map_err(|e| anyhow!(format!("{}", e)))
}

fn insert_impl(report: DailyReport, conn: &Transaction) -> anyhow::Result<usize> {
    let mut count = 0;
    let mut stmt = conn.prepare("INSERT OR IGNORE INTO hashes (id, hash) VALUES (?1, ?2)")?;
    for id_hash in report.killmails {
        stmt.execute(params![id_hash.get_id(), &id_hash.get_hash()[..]])?;
        count += 1;
    }
    return Ok(count);
}

fn query_hashes(count: u32, conn: &Connection) -> anyhow::Result<Vec<IdHash>> {
    let mut select_stmt = conn.prepare("SELECT id, hash FROM hashes WHERE state = 0 ORDER BY id DESC LIMIT ?1;")?;
    let mut rows = select_stmt.query([count])?;

    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let id: i32 = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        result.push((id, IdHashBinary::hash_to_string(&blob[..])));
    }
    Ok(result)
}

fn mark_complete(ids: &Vec<i32>, conn: &Connection) -> anyhow::Result<usize> {
    let mut stmt = conn.prepare("UPDATE hashes SET state = 1 WHERE id = ?1;")?;
    let mut count = 0;
    for id in ids {
        count += stmt.execute([id]).map_err(|e| anyhow!(e))?;
    }
    Ok(count)
}

fn save_handled_hash(id: i32, hash: String, conn: &Connection) -> anyhow::Result<()> {
    let mut stmt = conn.prepare("INSERT OR IGNORE INTO hashes (id, hash, state) VALUES (?1, ?2, 1)")?;
    let blob = IdHashBinary::string_to_hash(hash)?;
    stmt.execute(params![id, &blob])?;
    return Ok(());
}