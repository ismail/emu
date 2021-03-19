use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

use std::env;
use std::fs::File;
use std::io;
use std::io::prelude::*;
use std::io::{Error, ErrorKind};
use std::process::Command;

static ELF_MAGIC: [u8; 4] = [0x7f, 0x45, 0x4c, 0x46];

// https://refspecs.linuxfoundation.org/elf/gabi4+/ch4.eheader.html
// unsigned char   e_ident[16]
// uint16_t        e_type;
// uint16_t        e_machine;
const HEADER_SIZE: u8 = 16 + 2 + 2;

enum ELFClass {
    ELFCLASS32 = 1,
    ELFCLASS64,
}

#[derive(FromPrimitive)]
enum Machine {
    X86 = 3,
    ARM = 40,
    X86_64 = 62,
}

struct Executable {
    class: ELFClass,
    machine: Machine,
}

fn run_executable(executable: Executable, args: &Vec<String>) -> Result<(), io::Error> {
    let ld_suffix: &str;
    let lib_suffix: &str;
    let qemu_suffix: &str;
    let sysroot: &str = &env::var("SYSROOT").unwrap_or("".to_string());

    match executable.class {
        ELFClass::ELFCLASS32 => match executable.machine {
            Machine::ARM => {
                ld_suffix = "-armhf.so.3";
                lib_suffix = "";
                qemu_suffix = "arm";
            }
            Machine::X86 => {
                ld_suffix = ".so.2";
                lib_suffix = "";
                qemu_suffix = "i386";
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::Other,
                    "Invalid executable specification.",
                ))
            }
        },
        ELFClass::ELFCLASS64 => match executable.machine {
            Machine::ARM => {
                ld_suffix = "-aarch64.so.1";
                qemu_suffix = "aarch64";
                lib_suffix = "64";
            }
            Machine::X86_64 => {
                ld_suffix = "-x86_64.so.2";
                qemu_suffix = "x86_64";
                lib_suffix = "64";
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::Other,
                    "Invalid executable specification.",
                ))
            }
        }
    }

    if sysroot != "" {
        Command::new(format!("/usr/bin/qemu-{}", qemu_suffix))
            .arg(format!(
                "{}/lib{}/ld-linux{}",
                sysroot, lib_suffix, ld_suffix
            ))
            .arg("--library-path")
            .arg(format!(
                "{root}/usr/lib{suffix}:{root}/lib{suffix}",
                root = sysroot,
                suffix = lib_suffix
            ))
            .args(&args[1..])
            .status()
            .expect(format!("Unable to run /usr/bin/qemu-{}", qemu_suffix).as_str());
    } else {
        Command::new(format!("/usr/bin/qemu-{}", qemu_suffix))
            .args(&args[1..])
            .status()
            .expect(format!("Unable to run /usr/bin/qemu-{}", qemu_suffix).as_str());
    }

    Ok(())
}

fn get_executable(executable: &str) -> Result<Executable, io::Error> {
    let f = File::open(&executable)?;

    let mut buffer = [0; HEADER_SIZE as usize];
    let mut handle = f.take(HEADER_SIZE as u64);

    handle.read(&mut buffer)?;

    if buffer[..4] != ELF_MAGIC {
        return Err(Error::new(
            ErrorKind::Other,
            format!("{} is not an ELF file.", executable),
        ));
    }

    let machine_type_value: u16 = buffer[18] as u16 + buffer[19] as u16 * 256;
    let machine_type: Machine;

    match FromPrimitive::from_u16(machine_type_value) {
        Some(Machine::ARM) => machine_type = Machine::ARM,
        Some(Machine::X86) => machine_type = Machine::X86,
        Some(Machine::X86_64) => machine_type = Machine::X86_64,
        None => {
            return Err(Error::new(
                ErrorKind::Other,
                format!(
                    "{} is not an ARM, x86 or x86_64 executable, machine type: {}",
                    executable, machine_type_value,
                ),
            ));
        }
    };

    let elfclass = buffer[4];

    let exec = Executable {
        class: match elfclass {
            1 => ELFClass::ELFCLASS32,
            2 => ELFClass::ELFCLASS64,
            _ => return Err(Error::new(ErrorKind::Other, "Invalid ELF class.")),
        },
        machine: machine_type,
    };

    Ok(exec)
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let executable = get_executable(&args[1]).unwrap();

    run_executable(executable, &args).unwrap();

    Ok(())
}