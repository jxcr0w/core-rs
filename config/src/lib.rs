extern crate jedi;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_json;

use ::std::fs::File;
use ::std::path::Path;
use ::std::io::prelude::*;
use ::std::env;
use ::std::sync::RwLock;

use ::jedi::{JSONError, Value, Serialize, DeserializeOwned};

pub type TResult<T> = Result<T, JSONError>;

lazy_static! {
    /// create a static/global CONFIG var, and load it with our config data
    static ref CONFIG: RwLock<Value> = {
        match load_config() {
            Ok(x) => RwLock::new(x),
            Err(e) => {
                panic!("error loading config: {}", e);
            },
        }
    };
}

/// load/parse our config file, and return the parsed JSON value
fn load_config() -> TResult<Value> {
    let path_env = match env::var("TURTL_CONFIG_FILE") {
        Ok(x) => x,
        Err(_) => String::from("config.yaml"),
    };
    let path = Path::new(&path_env[..]);
    let mut file = File::open(&path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let data: Value = jedi::parse_yaml(&contents)?;
    Ok(data)
}

/// get a string value from our config
pub fn get<T: DeserializeOwned>(keys: &[&str]) -> TResult<T> {
    let guard = (*CONFIG).read().unwrap();
    jedi::get(keys, &guard)
        .map_err(|e| From::from(e))
}

/// Set a value into our heroic config
pub fn set<T: Serialize>(keys: &[&str], val: &T) -> TResult<()> {
    let mut guard = (*CONFIG).write().unwrap();
    jedi::set(keys, &mut guard, val)
        .map_err(|e| From::from(e))
}

fn deep_merge(val1: &mut Value, val2: &Value) -> TResult<Value> {
    if !val1.is_object() || !val2.is_object() {
        return Err(JSONError::InvalidKey(String::from("deep_merge() -- bad objects passed")));
    }

    {
        let obj1 = val1.as_object_mut().unwrap();
        let obj2 = val2.as_object().unwrap();
        for (key, val) in obj2 {
            if val.is_object() {
                let merged_val = {
                    let mut obj1_val = obj1.entry(key.clone()).or_insert(json!({}));
                    if !obj1_val.is_null() && !obj1_val.is_object() {
                        return Err(JSONError::InvalidKey(String::from("deep_merge() -- trying to merge an object into a non-object")));
                    }
                    deep_merge(&mut obj1_val, &val)?
                };
                obj1.insert(key.clone(), merged_val);
            } else {
                obj1.insert(key.clone(), val.clone());
            }
        }
    }
    Ok(val1.clone())
}

/// Merge a serializable object into the config object
pub fn merge<T: Serialize>(obj: &T) -> TResult<()> {
    let mut config_mut = (*CONFIG).write().unwrap();
    let val = jedi::to_val(obj)?;
    deep_merge(&mut config_mut, &val)?;
    Ok(())
}

