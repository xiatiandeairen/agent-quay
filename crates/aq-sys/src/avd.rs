//! Android 虚拟设备（AVD）：读配置 AVD 的镜像、机型、内存，扫描本机装了哪些系统镜像，按写的版本、机型挑镜像和机型配置。
//! aq 每次借用都新建自己的 AVD（放在 aq 状态目录，不进 ~/.android/avd），还时连同目录删掉；配置里的 AVD 只提供默认值，aq 不启动它。
//! 本机实测（2026-11-23，emulator 36.5.11）：avdmanager 建一个 AVD 约 0.65 秒，没启动前目录只有 4K；新建的 AVD 带 -read-only 能正常启动（API 35 约 20 秒）。

use std::path::{Path, PathBuf};

/// 配置 AVD 里 aq 要沿用的几项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvdConf {
    /// image.sysdir.1，如 system-images/android-35/google_apis/arm64-v8a/
    pub sysdir: String,
    /// hw.device.name，如 pixel_6
    pub device: String,
    /// hw.ramSize，如 4096 或 4096M；读不到时为空（不改新 AVD 的默认值）
    pub ram: String,
}

/// 一个系统镜像：system-images/android-<api>/<tag>/<abi>/。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub api: String,
    pub tag: String,
    pub abi: String,
}

impl Image {
    pub fn package(&self) -> String {
        format!("system-images;android-{};{};{}", self.api, self.tag, self.abi)
    }
    fn from_sysdir(s: &str) -> Option<Image> {
        let mut p = s.trim().trim_matches('/').split('/');
        if p.next()? != "system-images" {
            return None;
        }
        let api = p.next()?.strip_prefix("android-")?.to_string();
        Some(Image { api, tag: p.next()?.to_string(), abi: p.next()?.to_string() })
    }
}

/// avdmanager list device 里的一个机型配置；kind 是用途（手机平板、手表、电视、车机、桌面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub kind: &'static str,
}

/// 新建 AVD 用的镜像、机型、内存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub image: Image,
    pub device: String,
    pub ram: String,
}

/// 用户的 AVD 目录：ANDROID_AVD_HOME，其次 ANDROID_USER_HOME/avd，默认 ~/.android/avd。
pub fn user_avd_home() -> PathBuf {
    if let Ok(d) = std::env::var("ANDROID_AVD_HOME") {
        return PathBuf::from(d);
    }
    if let Ok(d) = std::env::var("ANDROID_USER_HOME") {
        return Path::new(&d).join("avd");
    }
    Path::new(&std::env::var("HOME").unwrap_or_default()).join(".android/avd")
}

/// 本机能跑的镜像 ABI。
pub fn host_abi() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64-v8a"
    } else {
        "x86_64"
    }
}

fn abi_label(abi: &str) -> &str {
    if abi == "arm64-v8a" {
        "arm64"
    } else {
        abi
    }
}

fn ini_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

/// 读一个 AVD 的配置（<home>/<name>.ini 里的 path 指向 AVD 目录，没有就用 <home>/<name>.avd）。
pub fn read_conf(home: &Path, name: &str) -> Result<AvdConf, String> {
    let ini = std::fs::read_to_string(home.join(format!("{name}.ini"))).unwrap_or_default();
    let dir = ini_value(&ini, "path").map(PathBuf::from).unwrap_or_else(|| home.join(format!("{name}.avd")));
    let cfg = dir.join("config.ini");
    let text = std::fs::read_to_string(&cfg).map_err(|e| format!("cannot read AVD {name} ({}): {e}", cfg.display()))?;
    let sysdir = ini_value(&text, "image.sysdir.1").ok_or_else(|| format!("AVD {name} has no image.sysdir.1 in {}", cfg.display()))?;
    let device = ini_value(&text, "hw.device.name").unwrap_or_else(|| "pixel_6".into());
    Ok(AvdConf { sysdir, device, ram: ini_value(&text, "hw.ramSize").unwrap_or_default() })
}

/// 扫描 <sdk>/system-images 下装好的镜像。
pub fn installed(sdk: &Path) -> Vec<Image> {
    let mut out = vec![];
    let sub = |p: &Path| -> Vec<(String, PathBuf)> {
        std::fs::read_dir(p)
            .map(|rd| {
                rd.flatten().filter(|e| e.path().is_dir()).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect()
            })
            .unwrap_or_default()
    };
    for (a, ap) in sub(&sdk.join("system-images")) {
        let Some(api) = a.strip_prefix("android-") else { continue };
        for (tag, tp) in sub(&ap) {
            for (abi, _) in sub(&tp) {
                out.push(Image { api: api.to_string(), tag: tag.clone(), abi });
            }
        }
    }
    out
}

fn kind(tag: &str) -> &'static str {
    ["wear", "tv", "automotive", "desktop"].into_iter().find(|k| tag.contains(k)).unwrap_or("phone")
}

fn kind_name(k: &str) -> &str {
    match k {
        "wear" => "Wear OS",
        "tv" => "TV",
        "automotive" => "Automotive",
        "desktop" => "Desktop",
        _ => "phone or tablet",
    }
}

/// 解析 `avdmanager list device` 的输出：每段有 `id: N or "<id>"`，专用机型另有 `Tag : android-wear` 等。
pub fn parse_profiles(text: &str) -> Vec<Profile> {
    let mut out: Vec<Profile> = vec![];
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("id:") {
            if let (Some(a), Some(b)) = (rest.find('"'), rest.rfind('"')) {
                if b > a {
                    out.push(Profile { id: rest[a + 1..b].to_string(), kind: "phone" });
                }
            }
        } else if let Some(tag) = line.strip_prefix("Tag").and_then(|r| r.trim_start().strip_prefix(':')) {
            if let Some(p) = out.last_mut() {
                p.kind = kind(tag.trim());
            }
        }
    }
    out
}

fn api_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect()
}

fn tag_rank(tag: &str) -> u8 {
    match tag {
        "google_apis" => 0,
        "google_apis_playstore" => 1,
        _ => 2,
    }
}

fn pick_image(v: &str, def: &Image, images: &[Image], host: &str) -> Result<Image, String> {
    let mut runnable: Vec<&str> = images.iter().filter(|i| i.abi == host).map(|i| i.api.as_str()).collect();
    runnable.sort_by_key(|a| api_key(a));
    runnable.dedup();
    let list = runnable.join(", ");
    let fit: Vec<&Image> = images.iter().filter(|i| i.api == v && i.abi == host).collect();
    if fit.contains(&def) {
        return Ok(def.clone());
    }
    if let Some(i) = fit.iter().min_by_key(|i| (tag_rank(&i.tag), i.tag.clone())) {
        return Ok((*i).clone());
    }
    let h = abi_label(host);
    let install = format!("To install: sdkmanager \"system-images;android-{v};google_apis;{host}\"");
    let other: Vec<&str> = images.iter().filter(|i| i.api == v).map(|i| abi_label(&i.abi)).collect();
    Err(if !other.is_empty() {
        format!(
            "Android API {v} is installed only as {}, which cannot run on this Mac ({h}); installed {h}: {list}. {install}",
            other.join(", ")
        )
    } else if let Some(full) = runnable.iter().find(|a| a.starts_with(&format!("{v}."))) {
        format!("no {h} Android API {v} system image on this Mac; installed: {list}. Write the full API level, e.g. android-emu@{full}")
    } else {
        format!("no {h} Android API {v} system image on this Mac; installed: {list}. {install}")
    })
}

/// 按写的版本、机型挑镜像和机型配置，不写的取配置 AVD 的；内存总是沿用配置 AVD（CASE-HOLD-19、CASE-HOLD-14、CASE-HOLD-16）。
/// profiles 只在写了机型时需要。错误文字给调用方看。
pub fn plan(
    spec: &aq_core::devspec::DeviceSpec,
    def: &AvdConf,
    images: &[Image],
    host: &str,
    profiles: Option<&[Profile]>,
) -> Result<Plan, String> {
    let def_img = Image::from_sysdir(&def.sysdir)
        .ok_or_else(|| format!("cannot read the system image of the [android-emu] avd in config.toml: {}", def.sysdir))?;
    let image = match &spec.version {
        None if images.contains(&def_img) => def_img,
        None => return Err(format!("the system image of the [android-emu] avd in config.toml is not installed: {}", def_img.package())),
        Some(v) => pick_image(v, &def_img, images, host)?,
    };
    if let Some(m) = &spec.model {
        let profiles = profiles.unwrap_or_default();
        let ik = kind(&image.tag);
        let example = |k: &str| profiles.iter().filter(|p| p.kind == k).map(|p| p.id.as_str()).take(3).collect::<Vec<_>>();
        let Some(p) = profiles.iter().find(|p| p.id == *m) else {
            return Err(format!(
                "no Android device profile named {m}; e.g. {}; full list: avdmanager list device -c",
                example("phone").join(", ")
            ));
        };
        if p.kind != ik {
            let e = example(ik).first().copied().unwrap_or("pixel_7").to_string();
            return Err(format!(
                "{m} is a {} profile and needs a {} system image; API {} here is a {} image ({}). Pick a {} profile, e.g. android-emu@{}:{e}",
                kind_name(p.kind),
                kind_name(p.kind),
                image.api,
                kind_name(ik),
                image.tag,
                kind_name(ik),
                image.api
            ));
        }
    }
    Ok(Plan { image, device: spec.model.clone().unwrap_or_else(|| def.device.clone()), ram: def.ram.clone() })
}

/// 把新建 AVD 的内存改成配置 AVD 的（avdmanager 按机型配置给默认值，如 pixel_6 是 1536M）。
pub fn set_ram(avd_dir: &Path, ram: &str) -> Result<(), String> {
    if ram.is_empty() {
        return Ok(());
    }
    let cfg = avd_dir.join("config.ini");
    let text = std::fs::read_to_string(&cfg).map_err(|e| format!("cannot read {}: {e}", cfg.display()))?;
    let mut lines: Vec<String> =
        text.lines().filter(|l| l.split_once('=').is_none_or(|(k, _)| k.trim() != "hw.ramSize")).map(String::from).collect();
    lines.push(format!("hw.ramSize={ram}"));
    std::fs::write(&cfg, lines.join("\n") + "\n").map_err(|e| format!("cannot write {}: {e}", cfg.display()))
}

/// aq 建的 AVD 名字（设备名 aq-…）只含字母、数字、点、连字符、下划线，防止拼出别的路径。
fn safe(name: &str) -> bool {
    name.starts_with("aq-") && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 删掉 aq 建的一个 AVD（目录和 .ini）；不存在时报错。
pub fn remove(home: &Path, name: &str) -> Result<(), String> {
    if !safe(name) {
        return Err(format!("not an aq AVD: {name}"));
    }
    let dir = home.join(format!("{name}.avd"));
    let ini = home.join(format!("{name}.ini"));
    if !dir.exists() && !ini.exists() {
        return Err(format!("no such device: avd:{name}"));
    }
    let _ = std::fs::remove_file(&ini);
    std::fs::remove_dir_all(&dir).or_else(|e| if dir.exists() { Err(format!("cannot delete {}: {e}", dir.display())) } else { Ok(()) })
}

/// aq 建过、目录还在的 AVD 名字。
pub fn names(home: &Path) -> Vec<String> {
    std::fs::read_dir(home)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".avd").map(String::from))
                .filter(|n| safe(n))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
