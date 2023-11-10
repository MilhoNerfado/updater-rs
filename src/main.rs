use axum::extract::State;
use axum::{extract::Path, routing::get, Router};
use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use crc_any::CRCu16;
use serialport;
use std::collections::VecDeque;
use std::io::Read;
use std::net::SocketAddr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;
use std::{fs, io};
use tokio::time::sleep;

const SERIAL_PORT: &str = "/dev/ttyUSB0";
const BAUD_RATE: u32 = 115_200;
const UART_MAT_SIZE: usize = 100;
const MAX_SIZE: usize = 160;

#[tokio::main]
async fn main() -> ! {
    let (tx, _): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();
    let (ser_tx, ser_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel();

    std::thread::spawn(move || serial_task(tx, ser_rx));

    let app = Router::new()
        .route("/", get(root))
        .route("/iot/:path", get(iot_update))
        .route("/mke/:path", get(mke_update))
        .route("/test", get(test_msg))
        .with_state(ser_tx);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();

    println!("All OK!");

    loop {
        sleep(Duration::from_millis(100)).await;
    }
}

async fn root() -> &'static str {
    "Hello world!"
}

async fn test_msg(State(tx): State<Sender<Vec<u8>>>) -> &'static str {
    let data = general_purpose::STANDARD.encode("dataInTesting");

    let mut crc = CRCu16::crc16xmodem();
    crc.digest(&data);
    let crc = crc.to_string();
    let crc = crc.trim_start_matches("0x");
    let Ok(crc) = u64::from_str_radix(crc, 16) else {
        return "Failed to parse hex crc number";
    };

    let msg = format!("[\"123\",{{\"k\":12,\"v\":[0,{crc},\"{data}\"]}}]\n").into_bytes();

    let Ok(_) = tx.send(msg) else {
        return "Failed to send to tx";
    };

    "Ok"
}

async fn iot_update(Path(path): Path<String>, State(tx): State<Sender<Vec<u8>>>) -> &'static str {
    let Ok((mut file, file_len)) = get_file(&path) else {
        return "Failed to open file";
    };

    let msg = format!("[\"123\",{{\"k\":14,\"v\":[{file_len}]}}]\n").into_bytes();

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

    sleep(Duration::from_millis(1_000)).await;

    for i in 0..encoded_data.len() {
        match encoded_data.pop_front() {
            Some(data) => {
                let mut crc = CRCu16::crc16xmodem();
                crc.digest(&data);
                let crc = crc.to_string();
                let crc = crc.trim_start_matches("0x");
                let Ok(crc) = u64::from_str_radix(crc, 16) else {
                    return "Failed to parse hex crc number";
                };

                let offset = i * MAX_SIZE;

                let msg = format!("[\"123\",{{\"k\":13,\"v\":[{offset},{crc},\"{data}\"]}}]\n")
                    .into_bytes();

                println!("{}", msg.len());

                let Ok(_) = tx.send(msg) else {
                    return "Failed to send to tx";
                };

                sleep(Duration::from_millis(600)).await;
            }
            None => break,
        }
    }

    "Updated IOT"
}
async fn mke_update(Path(path): Path<String>, State(tx): State<Sender<Vec<u8>>>) -> &'static str {
    let Ok((mut file, file_len)) = get_file(&path) else {
        return "Failed to open file";
    };

    let mut buffer = [0; MAX_SIZE];
    let mut encoded_data: VecDeque<String> = VecDeque::new();

    let mut file_crc = CRCu16::crc16xmodem();

    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                file_crc.digest(&buffer);
                let b64_encoded = general_purpose::STANDARD.encode(&buffer[..bytes_read]);
                encoded_data.push_back(b64_encoded);
            }
            Err(_) => return "Failed to read/encode",
        }
    }

    let file_crc = file_crc.to_string();
    let file_crc = file_crc.trim_start_matches("0x");
    let Ok(file_crc) = u64::from_str_radix(file_crc, 16) else {
        return "Failed to parse hex crc number";
    };

    for i in 0..encoded_data.len() {
        match encoded_data.pop_front() {
            Some(data) => {
                let mut crc = CRCu16::crc16xmodem();
                crc.digest(&data);
                let crc = crc.to_string();
                let crc = crc.trim_start_matches("0x");
                let Ok(crc) = u64::from_str_radix(crc, 16) else {
                    return "Failed to parse hex crc number";
                };
                let offset = i * MAX_SIZE;

                println!("data len: {}", data.chars().count());

                let msg = format!("[\"123\",{{\"k\":12,\"v\":[{offset},{crc},\"{data}\"]}}]\n")
                    .into_bytes();

                println!("msg len: {}", msg.len());

                let Ok(_) = tx.send(msg) else {
                    return "Failed to send to tx";
                };

                sleep(Duration::from_millis(1_000)).await;
            }
            None => {}
        };
    }

    let msg = format!("[\"1234\",{{\"k\":11,\"v\":[true,{file_len},{file_crc}]}}]\n").into_bytes();

    let Ok(_) = tx.send(msg) else {
        return "Failed o sendo to tx";
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
            std::thread::sleep(Duration::from_millis(700));
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
        match port.read_exact(&mut read_buffer) {
            Ok(_) => {
                content_string.push(read_buffer[0] as char);
                // println!(
                //     "Read something (read_buffer):<{}> (Content_string):<{}>",
                //     read_buffer[0] as char, content_string
                // );

                if content_string.chars().last() == Some('\n') {
                    content_string.pop();
                    println!("[{}] {}", Utc::now(), content_string);
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

        if let Ok(mut write_content) = receiver.try_recv() {
            while !write_content.is_empty() {
                let to_write: Vec<u8> = write_content
                    .drain(0..std::cmp::min(write_content.len(), UART_MAT_SIZE))
                    .collect();

                match port.write(to_write.as_slice()) {
                    Ok(_) => {
                        println!(
                            "[{}] !! SIM !!  Wrote Something <{:?}>",
                            Utc::now(),
                            String::from_utf8(to_write)
                        );
                    }
                    Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                        port = serial_connect(SERIAL_PORT, BAUD_RATE);
                        println!("Connected to serial");
                    }
                    Err(_) => {}
                };

                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
}
