use std::env;

fn parse_version_number(version: &str) -> u64 {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();

    while parts.len() < 4 {
        parts.push(0);
    }

    (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16) | parts[3]
}

fn main() {
    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let version = env::var("CARGO_PKG_VERSION").expect("missing package version");
    let version_number = parse_version_number(&version);

    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/app-icon.ico")
        .set("ProductName", "Tapper")
        .set("FileDescription", "Tapper")
        .set("InternalName", "Tapper.exe")
        .set("OriginalFilename", "Tapper.exe")
        .set_version_info(winres::VersionInfo::FILEVERSION, version_number)
        .set_version_info(winres::VersionInfo::PRODUCTVERSION, version_number);

    res.compile().expect("failed to compile Windows resources");
}
