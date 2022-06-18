use crate::AdbConnection;
use anyhow::Result;

impl AdbConnection {
    pub fn shell(&mut self, cmd: &str) -> Result<Vec<u8>> {
        let mut buf = vec![];
        for packet in self.shell_stream(cmd)? {
            buf.extend(packet);
        }
        Ok(buf)
    }

    pub fn shell_stream(&mut self, cmd: &str) -> Result<impl Iterator<Item = Vec<u8>>> {
        Ok(self.open_stream(&format!("shell:{}", cmd))?)
    }
}
