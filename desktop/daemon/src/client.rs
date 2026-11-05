//! Client side of the control socket (used by the CLI and tests).

use std::path::Path;

use anyhow::{anyhow, Context};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixStream,
    },
};

use crate::ipc::{Reply, Request};

pub struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
}

impl Client {
    pub async fn connect(socket: &Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(socket).await.with_context(|| {
            format!(
                "PhoneLink daemon is not running (no socket at {}).\nStart it with: systemctl --user start phonelinkd",
                socket.display()
            )
        })?;
        let (r, w) = stream.into_split();
        Ok(Self {
            lines: BufReader::new(r).lines(),
            writer: w,
        })
    }

    pub async fn send(&mut self, req: &Request) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(req)?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;
        Ok(())
    }

    /// Next reply, or `None` when the daemon closed the connection.
    pub async fn next(&mut self) -> anyhow::Result<Option<Reply>> {
        match self.lines.next_line().await? {
            Some(l) => Ok(Some(serde_json::from_str(&l)?)),
            None => Ok(None),
        }
    }

    pub async fn request(&mut self, req: &Request) -> anyhow::Result<Reply> {
        self.send(req).await?;
        self.next()
            .await?
            .ok_or_else(|| anyhow!("daemon closed the connection"))
    }
}
