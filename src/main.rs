//! hp4: print text and images on the HP4 Bluetooth label printer.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use clap::{Args, Parser, Subcommand};
use image::RgbImage;
use serde_json::json;

use open_hp4_printer::bt::{self, Printer};
use open_hp4_printer::media::{self, Kind, Media, MEDIA};
use open_hp4_printer::protocol::{self, Encoder, DOTS_PER_MM};
use open_hp4_printer::{build_job, raster, render};

#[derive(Parser)]
#[command(
    name = "hp4",
    version,
    about = "Print text and images on the HP4 Bluetooth label printer"
)]
struct Cli {
    /// Printer Bluetooth address (default: $HP4_ADDR or the saved one)
    #[arg(long, global = true)]
    addr: Option<String>,
    /// RFCOMM channel
    #[arg(long, global = true, default_value_t = 1)]
    channel: u8,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print one label per argument ("\n" starts a new line; reads stdin if none)
    Text {
        text: Vec<String>,
        /// TTF/OTF font file (default: built-in DejaVu Sans Bold)
        #[arg(long)]
        font: Option<String>,
        /// Font size in dots (default: fill the tape)
        #[arg(long)]
        size: Option<f32>,
        #[command(flatten)]
        opts: PrintOpts,
    },
    /// Print image files (left to right = along the tape)
    Image {
        #[arg(required = true)]
        images: Vec<PathBuf>,
        /// Rotate counter-clockwise first: 0, 90, 180 or 270
        #[arg(long, default_value_t = 0)]
        rotate: u32,
        #[command(flatten)]
        opts: PrintOpts,
    },
    /// Show serial, firmware, battery and status flags
    Status,
    /// Show or set the idle power-off time: never, 10m, 30m, 1h, 3h, 5h, 10h or e.g. 120m
    AutoOff { time: Option<String> },
    /// Feed tape by this many millimetres
    Feed { mm: f32 },
    /// Look for nearby printers
    Scan {
        #[arg(long, default_value_t = 15)]
        seconds: u32,
    },
    /// Remember the printer address
    PairSave { addr: String },
    /// List media presets
    Media,
}

#[derive(Args)]
struct PrintOpts {
    /// Media preset (see `hp4 media`)
    #[arg(long, default_value = "tape15")]
    media: String,
    /// light, normal, dark or 0..15
    #[arg(long, default_value = "normal")]
    density: String,
    #[arg(long, default_value_t = 1)]
    copies: usize,
    /// Blank tape before and after the content, mm (tape only)
    #[arg(long, default_value_t = 5.0)]
    margin: f32,
    /// Minimum label length, mm (tape only)
    #[arg(long, default_value_t = 0.0)]
    length: f32,
    /// Save a PNG of what will be printed
    #[arg(long)]
    preview: Option<PathBuf>,
    /// Write the raw printer bytes to a file instead of printing
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Build the job but do not connect
    #[arg(long)]
    dry_run: bool,
    /// Print even if the printer reports a problem
    #[arg(long)]
    force: bool,
    /// Seconds to wait for printing to finish
    #[arg(long, default_value_t = 15)]
    wait: u64,
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("hp4").join("config.json")
}

fn printer_addr(cli: &Cli) -> anyhow::Result<String> {
    if let Some(a) = cli.addr.clone().or_else(|| std::env::var("HP4_ADDR").ok()) {
        return Ok(a);
    }
    let cfg: serde_json::Value = std::fs::read(config_path())
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    match cfg["addr"].as_str() {
        Some(a) => Ok(a.to_string()),
        None => {
            bail!("no printer address: pass --addr, set HP4_ADDR, or run `hp4 pair-save <addr>`")
        }
    }
}

fn connect(cli: &Cli) -> anyhow::Result<Printer> {
    Printer::connect(&printer_addr(cli)?, cli.channel)
}

fn print(cli: &Cli, labels: Vec<RgbImage>, media: &Media, opts: &PrintOpts) -> anyhow::Result<()> {
    let density = protocol::density_level(&opts.density)
        .context("density must be light, normal, dark or 0..15")?;
    let fitted: Vec<RgbImage> = labels
        .iter()
        .map(|img| raster::add_margin(&raster::fit(img, media), media, opts.margin, opts.length))
        .collect();
    let labels: Vec<RgbImage> = (0..opts.copies.max(1))
        .flat_map(|_| fitted.iter().cloned())
        .collect();

    if let Some(path) = &opts.preview {
        let pages: Vec<_> = labels
            .iter()
            .map(|l| raster::to_rows(l).map(|r| raster::preview(&r)))
            .collect::<Result<_, _>>()?;
        let w = pages.iter().map(|p| p.width()).max().unwrap_or(1);
        let h: u32 = pages.iter().map(|p| p.height() + 4).sum();
        let mut sheet = image::GrayImage::from_pixel(w, h, image::Luma([255]));
        let mut y = 0;
        for p in &pages {
            image::imageops::overlay(&mut sheet, p, 0, y);
            y += p.height() as i64 + 4;
        }
        sheet.save(path)?;
        eprintln!("preview: {} ({} label(s))", path.display(), labels.len());
    }

    let n = labels.len();
    let build = |enc: &mut Encoder| -> anyhow::Result<Vec<Vec<u8>>> {
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| build_job(enc, l, media, density, i == 0, i == n - 1))
            .collect()
    };

    if let Some(path) = &opts.output {
        let jobs = build(&mut Encoder::default())?;
        std::fs::write(path, jobs.concat())?;
        eprintln!(
            "wrote {} bytes to {}",
            jobs.iter().map(Vec::len).sum::<usize>(),
            path.display()
        );
        return Ok(());
    }
    if opts.dry_run {
        return Ok(());
    }

    let mut pr = connect(cli)?;
    let info = pr.handshake()?;
    let st = pr.status(Duration::from_millis(1500))?.or(info.status);
    if let Some(st) = &st {
        let problems = st.problems();
        if !problems.is_empty() && !opts.force {
            bail!(
                "printer reports: {} (use --force to print anyway)",
                problems.join(", ")
            );
        }
    }
    for job in build(&mut pr.enc)? {
        pr.send(&job)?;
        std::thread::sleep(Duration::from_millis(200));
    }
    // wait for the printer to finish before dropping the link
    let deadline = Instant::now() + Duration::from_secs(opts.wait);
    let mut last = st;
    while Instant::now() < deadline {
        if let Some(s) = pr.status(Duration::from_secs(1))? {
            let done = !s.flag("printing");
            last = Some(s);
            if done {
                break;
            }
        }
    }
    println!(
        "{}",
        json!({"printed": n, "media": media.name, "status": last})
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Text {
            text,
            font,
            size,
            opts,
        } => {
            let media = media::find(&opts.media)
                .with_context(|| format!("unknown media {}", opts.media))?;
            let font = render::load_font(font.as_deref())?;
            let texts = if text.is_empty() {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                vec![s.trim_end_matches('\n').to_string()]
            } else {
                text.clone()
            };
            let labels = texts
                .iter()
                .map(|t| {
                    render::text_image(
                        &font,
                        &t.replace("\\n", "\n"),
                        media.across_dots(),
                        *size,
                        8,
                    )
                })
                .collect();
            print(&cli, labels, &media, opts)
        }
        Cmd::Image {
            images,
            rotate,
            opts,
        } => {
            let media = media::find(&opts.media)
                .with_context(|| format!("unknown media {}", opts.media))?;
            let labels = images
                .iter()
                .map(|p| {
                    let img = raster::flatten(
                        &image::open(p)
                            .with_context(|| format!("opening {}", p.display()))?
                            .to_rgba8(),
                    );
                    Ok(match rotate % 360 {
                        0 => img,
                        90 => image::imageops::rotate270(&img),
                        180 => image::imageops::rotate180(&img),
                        270 => image::imageops::rotate90(&img),
                        r => bail!("--rotate must be 0, 90, 180 or 270, not {r}"),
                    })
                })
                .collect::<anyhow::Result<_>>()?;
            print(&cli, labels, &media, opts)
        }
        Cmd::Status => {
            let mut pr = connect(&cli)?;
            let info = pr.handshake()?;
            let st = pr.status(Duration::from_millis(1500))?.or(info.status);
            let problems = st.as_ref().map(|s| s.problems());
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "serial": info.serial, "firmware": info.firmware, "status": st, "problems": problems,
                }))?
            );
            Ok(())
        }
        Cmd::AutoOff { time } => auto_off(&cli, time.as_deref()),
        Cmd::Feed { mm } => {
            let mut pr = connect(&cli)?;
            let f = pr.enc.feed((mm * DOTS_PER_MM).round() as u16);
            pr.send(&f)?;
            std::thread::sleep(Duration::from_secs(1));
            Ok(())
        }
        Cmd::Scan { seconds } => {
            let found: Vec<_> = bt::scan(*seconds)?
                .into_iter()
                .map(|(a, n)| json!({"addr": a, "name": n}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&found)?);
            Ok(())
        }
        Cmd::PairSave { addr } => {
            let path = config_path();
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&json!({"addr": addr.to_uppercase()}))?,
            )?;
            eprintln!("saved {} to {}", addr.to_uppercase(), path.display());
            Ok(())
        }
        Cmd::Media => {
            for m in MEDIA {
                match m.kind {
                    Kind::Tape => println!(
                        "{:9} {:.0} mm continuous tape, {} mm printable",
                        m.name, m.across, m.print_across
                    ),
                    Kind::Gap => println!(
                        "{:9} {}x{} mm labels, {}x{} mm printable",
                        m.name, m.across, m.length, m.print_across, m.print_length
                    ),
                }
            }
            Ok(())
        }
    }
}

fn auto_off(cli: &Cli, time: Option<&str>) -> anyhow::Result<()> {
    let units = match time {
        Some(t) => Some(protocol::auto_off_units(&t.to_lowercase()).context(
            "use never, 10m, 30m, 1h, 3h, 5h, 10h or a multiple of 10 minutes like 120m",
        )?),
        None => None,
    };
    if let Some(u) = units {
        let mut pr = connect(cli)?;
        pr.handshake()?;
        let f = pr.enc.auto_off(u);
        pr.send(&f)?;
        std::thread::sleep(Duration::from_secs(1));
        drop(pr);
        std::thread::sleep(Duration::from_secs(2)); // the printer needs a moment before a new link
    }
    // read back on a fresh link; the printer sometimes drops the connection after a setting change
    let mut st = None;
    for _ in 0..3 {
        match connect(cli).and_then(|mut pr| pr.status(Duration::from_secs(3))) {
            Ok(s) => {
                st = s;
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
    let st = st.context("printer did not report its status")?;
    let mins = st.auto_off_min;
    let mut out = json!({"auto_off_min": mins, "never": mins == Some(0)});
    if let Some(u) = units {
        let ok = mins == Some(u as u32 * 10);
        out["ok"] = json!(ok);
        println!("{out}");
        if !ok {
            std::process::exit(1);
        }
    } else {
        println!("{out}");
    }
    Ok(())
}
