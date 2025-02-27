//
// Copyright (c) 2021 ZettaScale Technology
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   The Zenoh Team, <zenoh@zettascale.tech>
//
use cdr::{CdrLe, Infinite};
use clap::{App, Arg};
use crossterm::{
    cursor::MoveToColumn,
    event::{Event, KeyCode, KeyEvent, KeyModifiers},
    ExecutableCommand,
};
//use futures::prelude::*;
use futures::{select, FutureExt};
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::io::{stdout, Write};
use tokio::sync::mpsc;
use zenoh::{config::Config, pubsub::Publisher};

#[derive(Serialize, PartialEq, Debug)]
struct Vector3 {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Serialize, PartialEq, Debug)]
struct Twist {
    linear: Vector3,
    angular: Vector3,
}

#[derive(Deserialize, PartialEq)]
struct Time {
    sec: i32,
    nanosec: u32,
}

#[derive(Deserialize, PartialEq)]
struct Log {
    stamp: Time,
    level: u8,
    name: String,
    msg: String,
    file: String,
    function: String,
    line: u32,
}

impl fmt::Display for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}.{}] [{}]: {}",
            self.stamp.sec, self.stamp.nanosec, self.name, self.msg
        )
    }
}

async fn pub_twist<'a>(publisher: &'a Publisher<'_>, linear: f64, angular: f64) {
    let twist = Twist {
        linear: Vector3 {
            x: linear,
            y: 0.0,
            z: 0.0,
        },
        angular: Vector3 {
            x: 0.0,
            y: 0.0,
            z: angular,
        },
    };

    write!(
        stdout(),
        "Publish on {} : {:?}\r\n",
        publisher.key_expr(),
        twist
    )
    .unwrap_or_default();
    let encoded = cdr::serialize::<_, _, CdrLe>(&twist, Infinite).unwrap();
    if let Err(e) = publisher.put(encoded).await {
        log::warn!("Error writing {}: {}", publisher.key_expr(), e);
    }
}

async fn del_twist<'a>(publisher: &'a Publisher<'_>) {
    write!(stdout(), "Delete on {}\r\n", publisher.key_expr()).unwrap_or_default();
    if let Err(e) = publisher.delete().await {
        log::warn!("Error deleting {}: {}", publisher.key_expr(), e);
    }
}

#[tokio::main]
async fn main() {
    // Initiate logging
    env_logger::init();

    let (config, cmd_vel, rosout, linear_scale, angular_scale) = parse_args();

    println!("Opening session...");
    let session = zenoh::open(config).await.unwrap();

    println!("Subscriber on {}", rosout);
    let subscriber = session.declare_subscriber(rosout).await.unwrap();

    let publisher = session.declare_publisher(cmd_vel).await.unwrap();

    // Keyboard event read loop, sending each to an async_std channel
    // Note: enable raw mode for direct processing of key pressed, without having to hit ENTER...
    // Unfortunately, this mode doesn't process new line characters on println!().
    // Thus write!(stdout(), "...\r\n") has to be used instead.
    crossterm::terminal::enable_raw_mode().unwrap();
    let (key_sender, mut key_receiver) = mpsc::channel::<Event>(10);
    let key_sender = key_sender.clone();

    tokio::spawn(async move {
        loop {
            match crossterm::event::read() {
                Ok(ev) => {
                    if let Err(e) = key_sender.send(ev).await {
                        log::warn!("Failed to push Key Event: {}", e);
                    }
                }
                Err(e) => {
                    log::warn!("Input error: {}", e);
                }
            }
        }
    });

    write!(stdout(), "Waiting commands with arrow keys or space bar to stop. Press on ESC, 'q' or CTRL+C to quit.\r\n").unwrap_or_default();
    write!(
        stdout(),
        "If an InfluxDB is storing publications, press 'd' to delete them all\r\n"
    )
    .unwrap_or_default();
    // Events management loop
    loop {
        select!(
            // On sample received by the subsriber
            sample = subscriber.recv_async() => {
                if let Ok(sample) = sample {
                    match cdr::deserialize_from::<_, Log, _>(std::io::Cursor::new(sample.payload().to_bytes()), cdr::size::Infinite) {
                        Ok(log) => {
                            println!("{}", log);
                            std::io::stdout().execute(MoveToColumn(0)).unwrap();
                        }
                        Err(e) => log::warn!("Error decoding Log: {}", e),
                    }
                }
            },

            // On keyboard event received from the tokio channel
            event = key_receiver.recv().fuse() => {
                match event {
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Up,
                        modifiers: _,
                    })) => pub_twist(&publisher, 1.0 * linear_scale, 0.0).await,
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Down,
                        modifiers: _,
                    })) => pub_twist(&publisher, -1.0 * linear_scale, 0.0).await,
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Left,
                        modifiers: _,
                    })) => pub_twist(&publisher, 0.0, 1.0 * angular_scale).await,
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Right,
                        modifiers: _,
                    })) => pub_twist(&publisher, 0.0, -1.0 * angular_scale).await,
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Char(' '),
                        modifiers: _,
                    })) => pub_twist(&publisher, 0.0, 0.0).await,
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Esc,
                        modifiers: _,
                    }))
                    | Some(Event::Key(KeyEvent {
                        code: KeyCode::Char('q'),
                        modifiers: _,
                    })) => break,
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Char('d'),
                        modifiers: _
                    })) => {
                        del_twist(&publisher).await
                    },
                    Some(Event::Key(KeyEvent {
                        code: KeyCode::Char('c'),
                        modifiers,
                    })) => {
                        if modifiers.contains(KeyModifiers::CONTROL) {
                            break;
                        }
                    }
                    Some(_) => (),
                    None => {
                        log::warn!("Channel closed");
                        break;
                    }
                }
            }
        );
    }

    // Stop robot at exit
    pub_twist(&publisher, 0.0, 0.0).await;

    subscriber.undeclare().await.unwrap();
    publisher.undeclare().await.unwrap();
    session.close().await.unwrap();

    crossterm::terminal::disable_raw_mode().unwrap();

    std::process::exit(0);
}

fn parse_args() -> (Config, String, String, f64, f64) {
    let args = App::new("zenoh-net sub example")
        .arg(
            Arg::from_usage("-m, --mode=[MODE]  'The zenoh session mode (peer by default).")
                .possible_values(["peer", "client"]),
        )
        .arg(Arg::from_usage(
            "-e, --connect=[ENDPOINT]...   'Endpoints to connect to.'",
        ))
        .arg(Arg::from_usage(
            "-l, --listen=[ENDPOINT]...   'Endpoints to listen on.'",
        ))
        .arg(Arg::from_usage(
            "-c, --config=[FILE]      'A configuration file.'",
        ))
        .arg(
            Arg::from_usage("--cmd_vel=[topic] 'The 'cmd_vel' ROS2 topic'")
                .default_value("rt/turtle1/cmd_vel"),
        )
        .arg(
            Arg::from_usage("--rosout=[topic] 'The 'rosout' ROS2 topic'")
                .default_value("rt/rosout"),
        )
        .arg(
            Arg::from_usage("-a, --angular_scale=[FLOAT] 'The angular scale.'")
                .default_value("2.0"),
        )
        .arg(Arg::from_usage("-x, --linear_scale=[FLOAT] 'The linear scale.").default_value("2.0"))
        .get_matches();

    let mut config = if let Some(conf_file) = args.value_of("config") {
        Config::from_file(conf_file).unwrap()
    } else {
        Config::default()
    };

    if let Some(endpoints) = args.values_of("connect") {
        config
            .connect
            .endpoints
            .set(
                endpoints
                    .collect::<Vec<&str>>()
                    .as_slice()
                    .iter()
                    .map(|e| e.parse().unwrap())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    if let Some(endpoints) = args.values_of("listen") {
        config
            .listen
            .endpoints
            .set(
                endpoints
                    .collect::<Vec<&str>>()
                    .as_slice()
                    .iter()
                    .map(|e| e.parse().unwrap())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }

    let cmd_vel = args.value_of("cmd_vel").unwrap().to_string();
    let rosout = args.value_of("rosout").unwrap().to_string();
    let linear_scale: f64 = args.value_of("linear_scale").unwrap().parse().unwrap();
    let angular_scale: f64 = args.value_of("angular_scale").unwrap().parse().unwrap();

    (config, cmd_vel, rosout, linear_scale, angular_scale)
}