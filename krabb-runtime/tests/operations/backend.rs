use serde_json::{Value, json};
use std::{fs, io::Write, path::Path, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpSocket, TcpStream},
};

pub const RESULT: &str = "Verified report: orchid dataset, 17 rows, checksum orchid-17.";

pub async fn serve() {
    let home = std::path::PathBuf::from(std::env::var_os("KRABB_HOME").unwrap());

    fs::create_dir_all(home.join("backend")).unwrap();

    let address = fs::read_to_string(home.join("backend.url"))
        .ok()
        .map(|url| url.trim_start_matches("http://").parse().unwrap())
        .unwrap_or_else(|| "127.0.0.1:0".parse().unwrap());

    let socket = TcpSocket::new_v4().unwrap();

    socket.set_reuseaddr(true).unwrap();
    socket.bind(address).unwrap();

    let listener = socket.listen(16).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    krabb_file::save(home.join("backend.url"), url.as_bytes()).unwrap();

    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let home = home.clone();
        let url = url.clone();

        tokio::spawn(async move {
            let _ = connection(stream, &home, &url).await;
        });
    }
}

async fn connection(stream: TcpStream, home: &Path, url: &str) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    let mut first = String::new();

    if stream.read_line(&mut first).await? == 0 {
        return Ok(());
    }

    let mut length = 0;
    let mut headers = first.len();

    loop {
        let mut line = String::new();

        if stream.read_line(&mut line).await? == 0 {
            return Ok(());
        }

        headers += line.len();

        assert!(headers <= 16 * 1024);

        if line == "\r\n" {
            break;
        }

        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().unwrap();
        }
    }

    assert!(length <= krabb_core::jsonl::MAX);

    let value = if first.starts_with("GET ") {
        json!({"name":"persistent local backend", "capabilities":{"streaming":false},
            "supportedInterfaces":[{"protocolBinding":"JSONRPC","url":format!("{url}/rpc")}]})
    } else {
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await?;
        let request: Value = serde_json::from_slice(&body).unwrap();
        let mut ledger = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("backend/ledger.jsonl"))?;
        writeln!(ledger, "{request}")?;
        ledger.sync_all()?;

        match request["method"].as_str().unwrap() {
            "SendMessage" => {
                assert_eq!(request["params"]["configuration"]["returnImmediately"], true);
                krabb_file::save(home.join("backend/accepted"), b"accepted")?;

                while home.join("backend/hold-start").exists()
                    && !home.join("backend/release-start").exists()
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }

            "GetTask" => assert_eq!(request["params"]["id"], "remote-task-1"),

            "CancelTask" => {
                assert_eq!(request["params"]["id"], "remote-task-1");
                krabb_file::save(home.join("backend/cancel-requested"), b"requested")?;

                while home.join("backend/hold-cancel").exists()
                    && !home.join("backend/release-cancel").exists()
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }

            method => panic!("Unexpected backend method {method}"),
        }

        let state =
            fs::read_to_string(home.join("backend/state")).unwrap_or_else(|_| "working".into());
        let state = match state.as_str() {
            "completed" => "TASK_STATE_COMPLETED",
            "cancelled" => "TASK_STATE_CANCELED",
            _ => "TASK_STATE_WORKING",
        };

        json!({"jsonrpc":"2.0", "id":request["id"], "result":{
            "id":"remote-task-1", "status":{"state":state,"message":{"parts":[{"text":RESULT}]}}
        }})
    };

    let body = serde_json::to_vec(&value)?;

    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: \
            close\r\n\r\n",
        body.len()
    );

    stream.get_mut().write_all(headers.as_bytes()).await?;
    stream.get_mut().write_all(&body).await?;
    stream.get_mut().shutdown().await
}
