use axum::extract::State;
use axum::{extract::Path, routing::get, Router};
use base64::{engine::general_purpose, Engine as _};
use crc_any::CRCu16;
use serialport;
use std::collections::VecDeque;
use std::io::Read;
use std::net::SocketAddr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::sleep;
use std::time::Duration;
use std::{fs, io};

const SERIAL_PORT: &str = "/dev/ttyUSB0";
const BAUD_RATE: u32 = 115_200;

const MAX_SIZE: usize = 256;

#[tokio::main]
async fn main() -> ! {
    let (tx, _): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();
    let (ser_tx, ser_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();

    std::thread::spawn(move || serial_task(tx, ser_rx));

    let app = Router::new()
        .route("/", get(root))
        .route("/iot/:path", get(iot_update))
        .route("/mke/:path", get(mke_update))
        .with_state(ser_tx);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();

    println!("All OK!");

    loop {
        sleep(Duration::from_millis(100));
    }
}

async fn root() -> &'static str {
    "Hello world!"
}

async fn iot_update(Path(path): Path<String>, State(tx): State<Sender<Vec<u8>>>) -> &'static str {
    let Ok((mut file, file_len)) = get_file(&path) else {
        return "Failed to open file";
    };

    let msg = format!("[\"123\",{{\"k\":14,\"v\":[{file_len}]}}]\r\n").into_bytes();

    let Ok(_) = tx.send(msg) else {
        return "Failed to send to tx";
    };

    let mut buffer = [0; MAX_SIZE];
    let mut encoded_data: VecDeque<String> = VecDeque::new();

    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                let b64_encoded = general_purpose::STANDARD.encode(&buffer[..bytes_read]);
                encoded_data.push_back(b64_encoded);
            }
            Err(_) => return "Failed to read/encode",
        }
    }

    sleep(Duration::from_millis(1_000));

    loop {
        match encoded_data.pop_front() {
            Some(data) => {
                let mut crc = CRCu16::crc16xmodem();
                crc.digest(&data);
                let crc = crc.to_string();
                let crc = crc.trim_start_matches("0x");
                let Ok(crc) = u64::from_str_radix(crc, 16) else {
                    return "Failed to parse hex crc number";
                };
                let msg =
                    format!("[\"123\",{{\"k\":13,\"v\":[0,{crc},\"{data}\"]}}]\r\n").into_bytes();

                println!("{}", msg.len());

                let Ok(_) = tx.send(msg) else {
                    return "Failed to send to tx";
                };

                return "Returned";

                sleep(Duration::from_millis(1_000));
            }
            None => break,
        }
    }

    "Updated IOT"
}
async fn mke_update(Path(path): Path<String>) -> &'static str {
    let Ok((_, _)) = get_file(&path) else {
        return "Failed to open file";
    };

    "Updated MKE"
}

fn get_file(path: &str) -> Result<(fs::File, u64), io::Error> {
    let file = fs::File::open(path)?;

    let length = file.metadata()?.len();

    Ok((file, length))
}

fn serial_connect(serial_port: &str, baud_rate: u32) -> Box<dyn serialport::SerialPort> {
    loop {
        let Ok(port) = serialport::new(serial_port, baud_rate)
            .timeout(Duration::from_millis(10))
            .open()
        else {
            sleep(Duration::from_millis(500));
            continue;
        };

        break port;
    }
}

fn serial_task(sender: Sender<Vec<u8>>, receiver: Receiver<Vec<u8>>) -> ! {
    let mut port = serial_connect(SERIAL_PORT, BAUD_RATE);
    let mut read_buffer = [0u8];
    let mut content_string = String::new();

    loop {
        if let Ok(write_content) = receiver.try_recv() {
            match port.write(write_content.as_slice()) {
                Ok(_) => {
                    println!(
                        "!! SIM !!  Wrote Something <{:?}>",
                        String::from_utf8(write_content)
                    );
                }
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                    port = serial_connect(SERIAL_PORT, BAUD_RATE);
                    println!("Connected to serial");
                }
                Err(_) => {}
            };
        }

        match port.read_exact(&mut read_buffer) {
            Ok(_) => {
                content_string.push(read_buffer[0] as char);
                // println!(
                //     "Read something (read_buffer):<{}> (Content_string):<{}>",
                //     read_buffer[0] as char, content_string
                // );

                if content_string.chars().last() == Some('\n') {
                    content_string.pop();
                    println!("{}", content_string);
                    let _ = sender.send(content_string.as_bytes().to_vec());
                    content_string.clear();
                }

                continue;
            }
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                port = serial_connect(SERIAL_PORT, BAUD_RATE);
                println!("Connected to serial");
            }
            Err(_) => {}
        }
    }
}
