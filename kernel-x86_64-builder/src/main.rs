fn main() {
    let kernel_path = std::env::args().nth(1).expect("usage: builder <kernel-elf-path> <out-dir>");
    let out_dir = std::env::args().nth(2).expect("usage: builder <kernel-elf-path> <out-dir>");
    let kernel_path = std::path::PathBuf::from(kernel_path);
    let out_dir = std::path::PathBuf::from(out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let uefi_path = out_dir.join("uosc-uefi.img");
    let bios_path = out_dir.join("uosc-bios.img");

    bootloader::UefiBoot::new(&kernel_path)
        .create_disk_image(&uefi_path)
        .expect("failed to create UEFI disk image");
    bootloader::BiosBoot::new(&kernel_path)
        .create_disk_image(&bios_path)
        .expect("failed to create BIOS disk image");

    println!("UEFI image: {}", uefi_path.display());
    println!("BIOS image: {}", bios_path.display());
}
