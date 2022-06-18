use crate::devices::{DeviceId, PartialRunner};
use crate::{Arch, Platform};
use adb_rs::push::AdbPush;
use adb_rs::{AdbClient, AdbConnection};
use anyhow::Result;
use apk::Apk;
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use std::collections::VecDeque;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct Adb {
    conn: AdbConnection,
    id: DeviceId,
}

impl Adb {
    pub fn devices(_devices: &mut [DeviceId]) -> Result<()> {
        // usb not supported yet
        // mdns not supported yet
        Ok(())
    }

    pub fn connect(device: String) -> Result<Self> {
        // TODO:
        // let private_key = mdb::adb::adbkey()?;
        let private_key = RsaPrivateKey::read_pkcs8_pem_file("/home/dvc/.android/adbkey").unwrap();
        Ok(Self {
            conn: AdbClient::new(private_key, "host::").connect(&device)?,
            id: DeviceId::Adb(device),
        })
    }

    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    pub fn push(&mut self, local_path: &Path, remote_path: &str) -> Result<()> {
        println!("push {} {}", local_path.display(), remote_path);
        let file_name = local_path.file_name().unwrap().to_str().unwrap();
        let remote_path = remote_path.trim_end_matches('/');
        let remote_path = format!("{}/{}", remote_path, file_name);
        self.conn.push(local_path, &remote_path)?;
        Ok(())
    }

    pub fn shell(&mut self, command: &str) -> Result<Vec<u8>> {
        println!("{}", command);
        self.conn.shell(command)
    }

    fn getprop(&mut self, prop: &str) -> Result<String> {
        let output = self.shell(&format!("getprop {}", prop))?;
        Ok(std::str::from_utf8(&output)?.trim().to_string())
    }

    fn install(&mut self, path: &Path) -> Result<()> {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        self.push(path, "/data/local/tmp".as_ref())?;
        self.shell(&format!("pm install /data/local/tmp/{}", file_name))?;
        Ok(())
    }

    /// To run a native activity use "android.app.NativeActivity" as the activity name
    fn start(&mut self, package: &str, activity: &str) -> Result<()> {
        self.shell(&format!(
            "am start -a android.intent.action.RUN -n {}/{}",
            package, activity
        ))?;
        Ok(())
    }

    fn stop(&mut self, id: &str) -> Result<()> {
        self.shell(&format!("am force-stop {}", id))?;
        Ok(())
    }

    fn set_debug_app(&mut self, package: &str) -> Result<()> {
        self.shell(&format!("am set-debug-app -w {}", package))?;
        Ok(())
    }

    fn clear_debug_app(&mut self) -> Result<()> {
        self.shell("am clear-debug-app")?;
        Ok(())
    }

    fn logcat_last_timestamp(&mut self) -> Result<String> {
        let output = self.shell("logcat -v time -t 1")?;
        let line = std::str::from_utf8(&output)?.lines().nth(1).unwrap();
        Ok(line[..18].to_string())
    }

    fn pidof(&mut self, id: &str) -> Result<u32> {
        loop {
            let output = self.shell(&format!("pidof {}", id))?;
            let pid = std::str::from_utf8(&output)?.trim();
            // may return multiple space separated pids if the old process hasn't exited yet.
            if pid.is_empty() || pid.split_once(' ').is_some() {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            return Ok(pid.parse()?);
        }
    }

    fn logcat(&mut self, pid: u32, last_timestamp: &str) -> Result<Logcat> {
        let iter = self
            .conn
            .shell_stream(&format!("logcat -T '{}' --pid={}", last_timestamp, pid))?;
        Ok(Logcat::new(iter))
    }

    pub fn forward(&mut self, _port: u16) -> Result<u16> {
        // TODO: impl forward
        todo!()
        /*let output = self
            .adb(device)
            .arg("forward")
            .arg("tcp:0")
            .arg(format!("tcp:{}", port))
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "adb forward exited with code {:?}",
            output.status.code()
        );
        Ok(std::str::from_utf8(&output.stdout)?.trim().parse()?)*/
    }

    /*fn app_dir(&self, device: &str, package: &str) -> Result<PathBuf> {
        let output = self
            .shell(device, Some(package))
            .arg("sh")
            .arg("-c")
            .arg("pwd")
            .output()?;
        anyhow::ensure!(output.status.success(), "failed to get app dir");
        Ok(Path::new(std::str::from_utf8(&output.stdout)?.trim()).to_path_buf())
    }*/

    pub fn lldb(&mut self, _lldb_server: &Path, _executable: &Path) -> Result<()> {
        // TODO: impl adb lldb
        todo!();
        /*/*let package = env.manifest().android().package.as_ref().unwrap();
        let app_dir = self.app_dir(device, package)?;
        self.shell(device, Some(package))
            .arg("chmod")
            .arg("a+x")
            .arg(&app_dir)
            .status()?;
        let dest = app_dir.join("lldb-server");*/
        self.push(lldb_server, "/data/local/tmp")?;
        /*self.shell(device, None)
            .arg("cat")
            .arg("/data/local/tmp/lldb-server")
            .arg("|")
            .arg("run-as")
            .arg(package)
            .arg("sh")
            .arg("-c")
            .arg(format!("'cat > {}'", dest.display()))
            .status()?;
        self.shell(device, Some(package))
            .arg("chmod")
            .arg("700")
            .arg(&dest)
            .status()?;*/
        let mut lldb_server = self
            .shell("cd /data/local/tmp && ./lldb-server platform --listen *:10086 --server")?;
            .spawn()?;
        std::thread::sleep(Duration::from_millis(100));
        self.forward(device, 10086)?;
        let status = Command::new("lldb")
            .arg("-O")
            .arg("platform select remote-android")
            //.arg("-O")
            //.arg(format!("platform settings -w {}", app_dir.display()))
            //.arg("platform settings -w /data/local/tmp")
            .arg("-O")
            .arg(format!("platform connect connect://{}:10086", device))
            .arg(executable)
            .status()?;
        anyhow::ensure!(status.success(), "lldb exited with nonzero exit code.");
        lldb_server.kill()?;
        Ok(())*/
    }

    pub fn run(&mut self, path: &Path, flutter_attach: bool, debug: bool) -> Result<PartialRunner> {
        let entry_point = Apk::entry_point(path)?;
        let package = &entry_point.package;
        let activity = &entry_point.activity;
        self.stop(package)?;
        if debug {
            self.set_debug_app(package)?;
        } else {
            self.clear_debug_app()?;
        }
        self.install(path)?;
        let last_timestamp = self.logcat_last_timestamp()?;
        self.start(package, activity)?;
        let pid = self.pidof(package)?;
        let mut logcat = self.logcat(pid, &last_timestamp)?;
        let url = if flutter_attach {
            let url = loop {
                if let Some(line) = logcat.next() {
                    if let Some((_, url)) = line.rsplit_once(' ') {
                        if url.starts_with("http") {
                            break url.trim().to_string();
                        }
                    }
                    println!("{}", line);
                }
            };
            Some(url)
        } else {
            None
        };
        Ok(PartialRunner {
            url,
            logger: Box::new(move || {
                for line in logcat {
                    println!("{}", line);
                }
            }),
            child: None,
        })
    }

    pub fn name(&mut self) -> Result<String> {
        self.getprop("ro.product.device")
    }

    pub fn platform(&self) -> Result<Platform> {
        Ok(Platform::Android)
    }

    pub fn arch(&mut self) -> Result<Arch> {
        let arch = match self.getprop("ro.product.cpu.abi")?.as_str() {
            "arm64-v8a" => Arch::Arm64,
            //"armeabi-v7a" => Arch::Arm,
            "x86_64" => Arch::X64,
            //"x86" => Arch::X86,
            abi => anyhow::bail!("unrecognized abi {}", abi),
        };
        Ok(arch)
    }

    pub fn details(&mut self) -> Result<String> {
        let release = self.getprop("ro.build.version.release")?;
        let sdk = self.getprop("ro.build.version.sdk")?;
        Ok(format!("Android {} (API {})", release, sdk))
    }
}

pub struct Logcat {
    stream: Box<dyn Iterator<Item = Vec<u8>> + Send>,
    lines: VecDeque<String>,
}

impl Logcat {
    fn new(stream: impl Iterator<Item = Vec<u8>> + Send + 'static) -> Self {
        Self {
            stream: Box::new(stream),
            lines: VecDeque::new(),
        }
    }
}

impl Iterator for Logcat {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(line) = self.lines.pop_front() {
                return Some(line);
            }
            let packet = self.stream.next()?;
            let packet = std::str::from_utf8(&packet).unwrap();
            for line in packet.split('\n') {
                let line = line.trim();
                if let Some((date, line)) = line.split_once(' ') {
                    if let Some((time, line)) = line.split_once(' ') {
                        if date.len() == 5 && time.len() == 12 {
                            self.lines.push_back(line.to_string());
                            continue;
                        }
                    }
                }
                self.lines.push_back(line.to_string());
            }
        }
    }
}
