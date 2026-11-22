//! The real CAT port: the TS-480's RS-232 COM connector, usually through a
//! USB-to-serial adapter.

use std::io::{self, Read, Write};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serialport::{ClearBuffer, DataBits, FlowControl, Parity, SerialPort, StopBits};

use super::Transport;

/// Port settings. They must match the rig's COM menu (TS-480 menu 56 sets
/// the baud rate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialSettings {
    /// `COM3` on Windows, `/dev/ttyUSB0` on Linux.
    pub port: String,
    pub baud: u32,
    pub two_stop_bits: bool,
    /// RTS/CTS hardware handshake.
    pub hardware_flow: bool,
    /// Level of the RTS line when hardware handshake is off. Some cables need
    /// it asserted for the rig to talk; interfaces that key the rig with RTS
    /// need it off.
    pub rts: bool,
    /// Level of the DTR line. Off by default because many interfaces use DTR
    /// to key the transmitter.
    pub dtr: bool,
}

impl Default for SerialSettings {
    fn default() -> Self {
        SerialSettings {
            port: String::new(),
            baud: 9600,
            two_stop_bits: false,
            hardware_flow: false,
            rts: true,
            dtr: false,
        }
    }
}

pub const BAUD_RATES: [u32; 5] = [4800, 9600, 19200, 38400, 57600];

pub struct SerialTransport {
    port: Box<dyn SerialPort>,
}

impl SerialTransport {
    pub fn open(s: &SerialSettings) -> io::Result<SerialTransport> {
        if s.port.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no CAT port selected (Settings → Rig)"));
        }
        let mut port = serialport::new(&s.port, s.baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(if s.two_stop_bits { StopBits::Two } else { StopBits::One })
            .flow_control(if s.hardware_flow { FlowControl::Hardware } else { FlowControl::None })
            .timeout(Duration::from_millis(10))
            .open()?;
        if !s.hardware_flow {
            port.write_request_to_send(s.rts)?;
        }
        port.write_data_terminal_ready(s.dtr)?;
        port.clear(ClearBuffer::All)?;
        log::info!("Opened CAT port {} at {} baud", s.port, s.baud);
        Ok(SerialTransport { port })
    }
}

impl Transport for SerialTransport {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.port.write_all(data)?;
        self.port.flush()
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e),
        }
    }
}

/// A serial port as offered in the settings list.
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    pub name: String,
    /// Adapter description, e.g. "FTDI USB Serial (COM3)".
    pub description: String,
}

pub fn list_ports() -> Vec<PortInfo> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let description = match &p.port_type {
                serialport::SerialPortType::UsbPort(usb) => {
                    let product = usb.product.clone().unwrap_or_else(|| "USB serial".into());
                    match &usb.manufacturer {
                        Some(m) if !product.contains(m.as_str()) => format!("{m} {product}"),
                        _ => product,
                    }
                }
                serialport::SerialPortType::BluetoothPort => "Bluetooth".into(),
                serialport::SerialPortType::PciPort => "Built-in".into(),
                serialport::SerialPortType::Unknown => String::new(),
            };
            PortInfo { name: p.port_name, description }
        })
        .collect();
    // COM10 after COM9, not after COM1.
    ports.sort_by_key(|p| {
        let digits: String = p.name.chars().filter(char::is_ascii_digit).collect();
        (p.name.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned(), digits.parse::<u32>().unwrap_or(0))
    });
    ports
}
