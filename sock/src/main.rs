#[macro_use]
extern crate log;
extern crate turtl_core;
extern crate websocket;

use ::std::thread;
use ::websocket::{Message, OwnedMessage};
use ::websocket::sync::Server;
use ::std::time::Duration;
use ::std::env;
use ::std::sync::{Arc, RwLock};

/// Go to sleeeeep
pub fn sleep(millis: u64) {
    thread::sleep(Duration::from_millis(millis));
}

pub fn main() {
    if env::var("TURTL_CONFIG_FILE").is_err() {
        env::set_var("TURTL_CONFIG_FILE", "../config.yaml");
    }
    turtl_core::init().unwrap();
    let handle = turtl_core::start(String::from(r#"{"messaging":{"reqres_append_mid":false}}"#));
    let server = Server::bind("127.0.0.1:7472").unwrap();
    info!("* sock server bound, listening");
    let conn_id: Arc<RwLock<u32>> = Arc::new(RwLock::new(0));
    macro_rules! inc_conn_id {
        ($conn:expr) => {
            {
                let mut guard = $conn.write().unwrap();
                *guard += 1;
                *guard
            }
        }
    }
    macro_rules! get_conn_id {
        ($conn:expr) => {
            {
                let guard = $conn.read().unwrap();
                *guard
            }
        }
    }
    for connection in server.filter_map(Result::ok) {
        let cid = conn_id.clone();
        let this_conn_id = inc_conn_id!(cid);
        thread::spawn(move || {
            info!("* new connection! {}", get_conn_id!(cid));
            let mut client = connection.accept().unwrap();
            client.set_nonblocking(true).unwrap();
            turtl_core::send(String::from(r#"["0","sync:shutdown",false]"#)).unwrap();
            turtl_core::send(String::from(r#"["0","user:logout",false]"#)).unwrap();
            loop {
                // make sure that if our stupid lazy connection has been left
                // behind that it is forgotten forever and ever and ever and
                // ever and ever.
                if this_conn_id != get_conn_id!(cid) { break; }

                let msg_res = client.recv_message();
                match msg_res {
                    Ok(msg) => {
                        match msg {
                            OwnedMessage::Close(_) => { break; }
                            OwnedMessage::Binary(x) => {
                                info!("* ui -> core ({})", x.len());
                                let msg_str = String::from_utf8(x).unwrap();
                                turtl_core::send(msg_str).unwrap();
                            }
                            OwnedMessage::Text(x) => {
                                info!("* ui -> core ({})", x.len());
                                turtl_core::send(x).unwrap();
                            }
                            _ => {}
                        }
                    }
                    Err(_) => {
                    }
                }

                let msg_turtl = turtl_core::recv_nb(None).unwrap();
                match msg_turtl {
                    Some(x) => {
                        info!("* core -> ui ({})", x.len());
                        //println!("---\n{}", x);
                        client.send_message(&Message::text(x)).unwrap();
                    }
                    None => {}
                }

                let msg_turtl = turtl_core::recv_event_nb().unwrap();
                match msg_turtl {
                    Some(x) => {
                        info!("* core -> ui ({})", x.len());
                        //println!("---\n{}", x);
                        client.send_message(&Message::text(x)).unwrap();
                    }
                    None => {}
                }
                sleep(10);
            }
            info!("* connection ended! {}", this_conn_id);
        });
    }
    handle.join().unwrap();
}

