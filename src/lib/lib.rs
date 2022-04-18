use hex;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::convert::TryInto;

type Hash = [u8; 20];
pub type IdHash = (i32, String);

pub const CMD_TOPIC: &'static str = "zkb/commands";
pub const DATA_TOPIC: &'static str = "zkb/data";

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum CmdEvent {
    SaveDailyReport(DailyReport),
    ReturnHash(IdHash),
    RequestLastHashes(u32),
    MarkComplete(Vec<i32>),
    SaveHandledHash(IdHash),
    Quit,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub enum DataEvent {
    HashesToHandle(Vec<IdHash>),
    KillmailToStore(Killmail),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct DailyReport {
    pub date: String,
    pub killmails: Vec<IdHashBinary>,
}
impl DailyReport {
    pub fn new(day: String) -> Self {
        Self {
            date: day,
            killmails: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct IdHashBinary {
    id: i32,
    hash: Hash,
}
impl IdHashBinary {
    const ERR_DECODE: &'static str = "Can't decode hash to binary";
    const ERR_ARRAY: &'static str = "Unexpected length of the vector";

    pub fn get_id(&self) -> i32 {
        self.id
    }

    pub fn get_hash(&self) -> Hash {
        self.hash
    }

    pub fn hash_to_string(hash: &[u8]) -> String {
        hex::encode(hash)
    }

    pub fn string_to_hash(hash: String) -> anyhow::Result<Vec<u8>> {
        hex::decode(hash).map_err(|e| anyhow::anyhow!(e))
    }
}
impl TryFrom<(&i32, &String)> for IdHashBinary {
    type Error = &'static str;

    fn try_from((id, hash): (&i32, &String)) -> Result<Self, Self::Error> {
        let id = *id;
        let binary = hex::decode(hash).or(Err(Self::ERR_DECODE))?;
        let hash: Hash = binary.try_into().or(Err(Self::ERR_ARRAY))?;
        Ok(Self { id, hash })
    }
}
impl TryFrom<(i32, &str)> for IdHashBinary {
    type Error = &'static str;

    fn try_from((id, hash): (i32, &str)) -> Result<Self, Self::Error> {
        IdHashBinary::try_from((&id, &hash.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_try_from_fail_on_invalid_hex_number() {
        let value = IdHashBinary::try_from((42, "1a3"));
        assert!(value.is_err());
        assert_eq!(value.unwrap_err(), IdHashBinary::ERR_DECODE);
    }

    #[test]
    fn test_try_from_fail_on_too_short_hash() {
        let value = IdHashBinary::try_from((42, "1a38d4921711476e5ea304f799a1552b4d2e5d"));
        assert!(value.is_err());
        assert_eq!(value.unwrap_err(), IdHashBinary::ERR_ARRAY);
    }

    #[test]
    fn test_try_from_fail_on_too_long_hash() {
        let value = IdHashBinary::try_from((42, "1a38d4921711476e5ea304f799a1552b4d2e5d2828"));
        assert!(value.is_err());
        assert_eq!(value.unwrap_err(), IdHashBinary::ERR_ARRAY);
    }

    #[test]
    fn test_try_from() {
        let res = IdHashBinary::try_from((42, "1a38d4921711476e5ea304f799a1552b4d2e5d28"));
        assert!(res.is_ok());
        let value = res.unwrap();
        assert_eq!(42, value.get_id());
        assert_eq!(
            "1a38d4921711476e5ea304f799a1552b4d2e5d28",
            IdHashBinary::hash_to_string(&value.get_hash()[..])
        );
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Killmail {
    pub killmail_id: i32,
    pub killmail_time: String,
    pub solar_system_id: i32,
    pub victim: Victim,
    pub attackers: Vec<Attackers>,
    pub zkb: Option<Zkb>
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Attackers {
    pub alliance_id: Option<i32>,
    pub character_id: Option<i32>,
    pub corporation_id: Option<i32>,
    pub damage_done: i32,
    pub ship_type_id: Option<i32>,
    pub weapon_type_id: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Victim {
    pub alliance_id: Option<i32>,
    pub character_id: Option<i32>,
    pub corporation_id: Option<i32>,
    pub damage_taken: i32,
    pub ship_type_id: Option<i32>
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Zkb {
    pub hash: String,
}


#[cfg(test)]
mod tests_killmail {
    use super::*;
    use std::io::prelude::*;
    use std::fs::File;

    #[test]
    fn test_killmail_deserialize() {
        let maybe_file = File::open("doc/killmail.json");
        assert!(maybe_file.is_ok());
        let mut buffer = String::new();
        assert!(maybe_file.unwrap().read_to_string(&mut buffer).is_ok());
        let maybe_killmail = serde_json::from_str::<Killmail>(&buffer);
        assert!(maybe_killmail.is_ok());
        let killmail = maybe_killmail.unwrap();

        assert_eq!(killmail.killmail_id, 97318112);
        assert_eq!(killmail.attackers.len(), 7);
        assert_eq!(killmail.victim.character_id, Some(308241937));

    }

    #[test]
    fn test_zkb_killmail_deserialize() {
        let maybe_file = File::open("doc/zkb.json");
        assert!(maybe_file.is_ok());
        let mut buffer = String::new();
        assert!(maybe_file.unwrap().read_to_string(&mut buffer).is_ok());
        let maybe_killmail = serde_json::from_str::<Killmail>(&buffer);
        assert!(maybe_killmail.is_ok());
        let killmail = maybe_killmail.unwrap();

        assert_eq!(killmail.killmail_id, 98190688);
        assert_eq!(killmail.attackers.len(), 1);
        assert_eq!(killmail.victim.character_id, Some(2118847117));
        assert!(killmail.zkb.is_some());
        assert_eq!(killmail.zkb.unwrap().hash, String::from("9377f28e34eabc18162e57e7e85f7a15c9339604"));

    }
}