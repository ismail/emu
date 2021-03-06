use num_enum::TryFromPrimitive;
use std::convert::TryFrom;

use std::env;
use std::fs::File;
use std::io;
use std::io::prelude::*;
use std::io::SeekFrom;
use std::path::Path;
use std::process::Command;
use std::str;

static ELF_MAGIC: [u8; 4] = [0x7f, 0x45, 0x4c, 0x46];

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, TryFromPrimitive)]
#[repr(u8)]
enum ELFClass {
    ELFCLASS32 = 1,
    ELFCLASS64,
}

#[derive(Debug, TryFromPrimitive)]
#[repr(u8)]
enum Endian {
    Little = 1,
    Big,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, TryFromPrimitive)]
#[repr(u16)]
enum Machine {
    X86 = 3,
    PPC64 = 21,
    S390 = 22,
    ARM = 40,
    X86_64 = 62,
    AARCH64 = 183,
    RISCV = 243,
}

struct Executable {
    class: ELFClass,
    endian: Endian,
    loader: String,
    machine: Machine,
}

macro_rules! unpack {
    ($bytes:expr, $inttype:ty, $endian:expr) => {
        match $endian {
            Endian::Little => <$inttype>::from_le_bytes($bytes),
            Endian::Big => <$inttype>::from_be_bytes($bytes),
        }
    };
}

fn run_executable(executable: Executable, args: &[String]) {
    let qemu_suffix: &str = match executable.machine {
        Machine::AARCH64 => "aarch64",
        Machine::ARM => "arm",
        Machine::PPC64 => match executable.endian {
            Endian::Big => "ppc64",
            Endian::Little => "ppc64le",
        },
        Machine::RISCV => match executable.class {
            ELFClass::ELFCLASS32 => "riscv32",
            ELFClass::ELFCLASS64 => "riscv64",
        },
        Machine::S390 => match executable.class {
            ELFClass::ELFCLASS32 => "s390",
            ELFClass::ELFCLASS64 => "s390x",
        },
        Machine::X86 => "i386",
        Machine::X86_64 => "x86_64",
    };

    // On Ubuntu executables are named as qemu-<arch>-static
    let mut static_suffix: &str = "";
    let qemu_static_path = format!("/usr/bin/qemu-{}-static", qemu_suffix);
    if Path::new(&qemu_static_path).exists() {
        static_suffix = "-static";
    }

    let sysroot = env::var("EMU_SYSROOT").unwrap_or_default();
    if !sysroot.is_empty() {
        //println!("Sysroot: {}, Loader: {}", sysroot, executable.loader);

        if executable.loader.is_empty() {
            panic!(
                "EMU_SYSROOT is set to {} but this executable defines no loader.\n \
                This can't work, please unset EMU_SYSROOT variable and re-run the command.",
                sysroot
            );
        }

        // Sanity check
        let loader = format!("{}/{}", sysroot, executable.loader);
        if !Path::new(&loader).exists() {
            panic!(
                "{} does not exist, {} is not setup correctly.",
                executable.loader, sysroot
            );
        }

        Command::new(format!("/usr/bin/qemu-{}{}", qemu_suffix, static_suffix))
            .arg("-R")
            .arg("0xf7000000")
            .arg("-cpu")
            .arg("max")
            .arg(format!("{}/{}", sysroot, &executable.loader))
            .arg("--library-path")
            .arg(format!(
                "{root}/usr/lib{suffix}:{root}/lib{suffix}",
                root = sysroot,
                suffix = match executable.class {
                    ELFClass::ELFCLASS64 => "64",
                    _ => "",
                }
            ))
            .args(&args[1..])
            .status()
            .unwrap_or_else(|_| {
                panic!(
                    "Unable to run /usr/bin/qemu-{}{} using {} as sysroot.",
                    qemu_suffix, static_suffix, sysroot
                )
            });
    } else {
        // If there is no sysroot then the loader should exist in the filesystem.
        // Check that and error otherwise.

        if !executable.loader.is_empty() && !Path::new(&executable.loader).exists() {
            panic!("{}", format!("{} does not exist, consider setting EMU_SYSROOT variable to a working sysroot path.", executable.loader));
        }

        Command::new(format!("/usr/bin/qemu-{}{}", qemu_suffix, static_suffix))
            .arg("-R")
            .arg("0xf7000000")
            .arg("-cpu")
            .arg("max")
            .args(&args[1..])
            .status()
            .unwrap_or_else(|_| {
                panic!(
                    "Unable to run /usr/bin/qemu-{}{}",
                    qemu_suffix, static_suffix
                )
            });
    }
}

fn setup_executable(executable: &str) -> Result<Executable, io::Error> {
    let mut f = File::open(&executable)?;

    // https://man7.org/linux/man-pages/man5/elf.5.html
    //  #define EI_NIDENT 16

    // typedef struct {
    //      unsigned char e_ident[EI_NIDENT];
    //      uint16_t      e_type;
    //      uint16_t      e_machine;
    //      uint32_t      e_version;
    //      ElfN_Addr     e_entry; (uint32_t or uint64_t)
    //      ElfN_Off      e_phoff; (uint32_t or uint64_t)
    //      uint32_t      e_flags;
    //      uint16_t      e_ehsize;
    //      uint16_t      e_phentsize;
    //      uint16_t      e_phnum;
    //      uint16_t      e_shentsize;
    //      uint16_t      e_shnum;
    //      uint16_t      e_shstrndx;
    // } ElfN_Ehdr;

    let mut e_ident = [0; 16];

    // Read the elf magic
    f.read_exact(&mut e_ident)?;
    if e_ident[..4] != ELF_MAGIC {
        panic!("{} is not an ELF file.", executable);
    }

    // EI_CLASS
    let exec_class = ELFClass::try_from(e_ident[4]).unwrap_or_else(|_| {
        panic!("Invalid ELF class.");
    });

    // EI_DATA
    let exec_endian = Endian::try_from(e_ident[5]).unwrap_or_else(|_| {
        panic!("Unknown endianness.");
    });

    // Skip e_type
    f.seek(SeekFrom::Current(2))?;
    let mut e_machine = [0; 2];
    f.read_exact(&mut e_machine)?;

    let machine_type_value: u16 = unpack!(e_machine, u16, &exec_endian);
    let exec_machine = Machine::try_from(machine_type_value).unwrap_or_else(|_| {
        panic!(
            "{} is not a supported executable, machine type: {}",
            executable, machine_type_value
        )
    });

    let pheader_offset: u64;
    let pheader_size: u16;

    match exec_class {
        ELFClass::ELFCLASS32 => {
            let mut e_phoff = [0; 4];
            // Skip e_version + e_entry
            f.seek(SeekFrom::Current(4 + 4))?;
            f.read_exact(&mut e_phoff)?;
            pheader_offset = unpack!(e_phoff, u32, &exec_endian).into();

            let mut e_phentsize = [0; 2];
            // Skip e_shoff + e_flags + e_ehsize
            f.seek(SeekFrom::Current(4 + 4 + 2))?;
            f.read_exact(&mut e_phentsize)?;
            pheader_size = unpack!(e_phentsize, u16, &exec_endian);
        }
        ELFClass::ELFCLASS64 => {
            let mut e_phoff = [0; 8];
            // Skip e_version + e_entry
            f.seek(SeekFrom::Current(4 + 8))?;
            f.read_exact(&mut e_phoff)?;
            pheader_offset = unpack!(e_phoff, u64, &exec_endian);

            let mut e_phentsize = [0; 2];
            // Skip e_shoff + e_flags + e_ehsize
            f.seek(SeekFrom::Current(8 + 4 + 2))?;
            f.read_exact(&mut e_phentsize)?;
            pheader_size = unpack!(e_phentsize, u16, &exec_endian);
        }
    }

    let ph_num: u16;
    let mut e_phnum = [0; 2];
    f.read_exact(&mut e_phnum)?;
    ph_num = unpack!(e_phnum, u16, &exec_endian);

    /*
    typedef struct {
        uint32_t   p_type;
        Elf32_Off  p_offset;
        Elf32_Addr p_vaddr;
        Elf32_Addr p_paddr;
        uint32_t   p_filesz;
        uint32_t   p_memsz;
        uint32_t   p_flags;
        uint32_t   p_align;
    } Elf32_Phdr;

    typedef struct {
        uint32_t   p_type;
        uint32_t   p_flags;
        Elf64_Off  p_offset;
        Elf64_Addr p_vaddr;
        Elf64_Addr p_paddr;
        uint64_t   p_filesz;
        uint64_t   p_memsz;
        uint64_t   p_align;
    } Elf64_Phdr;
    */

    f.seek(SeekFrom::Start(pheader_offset))?;
    let mut i = 0;
    let mut header_type: u32;
    let mut p_type = [0; 4];
    let mut load_address: u64 = 0;
    let mut load_flags: u32;
    let mut virtual_address: u64 = 0;
    let mut interpreter_size: u64 = 0;
    let mut exec_loader: String = String::new();

    const PT_LOAD: u32 = 1;
    const PT_INTERP: u32 = 3;

    const PF_X: u32 = 1 << 0;
    const PF_R: u32 = 1 << 2;
    const PF_RX: u32 = PF_R | PF_X;

    while i < ph_num {
        f.read_exact(&mut p_type)?;

        header_type = unpack!(p_type, u32, &exec_endian);

        if header_type == PT_LOAD {
            let mut p_flags = [0; 4];

            match exec_class {
                ELFClass::ELFCLASS32 => {
                    let mut p_vaddr = [0; 4];
                    // Skip p_offset
                    f.seek(SeekFrom::Current(4))?;
                    f.read_exact(&mut p_vaddr)?;
                    let load_address_maybe: u32 = unpack!(p_vaddr, u32, &exec_endian);
                    // Skip p_paddr + p_filesz + p_memsz
                    f.seek(SeekFrom::Current(4 + 4 + 4))?;
                    f.read_exact(&mut p_flags)?;
                    load_flags = unpack!(p_flags, u32, &exec_endian);

                    if load_flags == PF_R || load_flags == PF_RX {
                        load_address = load_address_maybe.into();
                        break;
                    }
                }
                ELFClass::ELFCLASS64 => {
                    let mut p_vaddr = [0; 8];
                    f.read_exact(&mut p_flags)?;
                    load_flags = unpack!(p_flags, u32, &exec_endian);

                    // Skip p_offset
                    f.seek(SeekFrom::Current(8))?;
                    f.read_exact(&mut p_vaddr)?;
                    let load_address_maybe: u64 = unpack!(p_vaddr, u64, &exec_endian);

                    if load_flags == PF_R || load_flags == PF_RX {
                        load_address = load_address_maybe;
                        break;
                    }
                }
            }
        } else if header_type == PT_INTERP {
            match exec_class {
                ELFClass::ELFCLASS32 => {
                    let mut p_vaddr = [0; 4];
                    // Skip p_offset
                    f.seek(SeekFrom::Current(4))?;
                    f.read_exact(&mut p_vaddr)?;
                    virtual_address = unpack!(p_vaddr, u32, &exec_endian).into();

                    let mut p_filesz = [0; 4];
                    // Skip p_vaddr + p_paddr
                    f.seek(SeekFrom::Current(4 + 4))?;
                    f.read_exact(&mut p_filesz)?;
                    interpreter_size = unpack!(p_filesz, u32, &exec_endian).into();
                }
                ELFClass::ELFCLASS64 => {
                    let mut p_vaddr = [0; 8];
                    // Skip p_flags + p_offset
                    f.seek(SeekFrom::Current(4 + 8))?;
                    f.read_exact(&mut p_vaddr)?;
                    virtual_address = unpack!(p_vaddr, u64, &exec_endian);

                    let mut p_filesz = [0; 8];
                    // Skip p_paddr
                    f.seek(SeekFrom::Current(8))?;
                    f.read_exact(&mut p_filesz)?;
                    interpreter_size = unpack!(p_filesz, u64, &exec_endian);
                }
            }
        }

        i += 1;
        f.seek(SeekFrom::Start(pheader_offset + (i * pheader_size) as u64))?;
    }

    if interpreter_size != 0 {
        // interpreter is null terminated
        interpreter_size -= 1;

        f.seek(SeekFrom::Start(virtual_address - load_address))?;
        let mut interpreter: Vec<u8> = Vec::with_capacity(interpreter_size as usize);
        f.take(interpreter_size).read_to_end(&mut interpreter)?;

        exec_loader = str::from_utf8(&interpreter).unwrap().to_string();
    }

    //println!("Loader: {}", exec_loader);

    let exec = Executable {
        class: exec_class,
        endian: exec_endian,
        loader: exec_loader,
        machine: exec_machine,
    };

    Ok(exec)
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        println!("Usage: {} program <args>", args[0]);
        return;
    }

    let executable = setup_executable(&args[1]).unwrap();
    run_executable(executable, &args);
}
