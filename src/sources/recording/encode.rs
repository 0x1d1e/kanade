//! Which encoder records, and how it is told (ADR 0116, Update): the quality setting as the number
//! each encoder takes, the render nodes a VA-API encode may use, the choice between VA-API, NVENC
//! and software from what a probe found, and the arguments that follow from it for `wf-recorder`,
//! the window's GStreamer pipeline and the probes. Pure data and text: the probes themselves, which
//! run programs and read `/sys`, are in `probe`, behind `Probe`.

use std::fmt;
use std::time::{Duration, Instant};

pub use crate::config::{Preference, Quality};

pub const PROBE_BUDGET: Duration = Duration::from_millis(3000);
pub const TRIAL: Duration = Duration::from_millis(1500);
pub const MAX_VAAPI_CANDIDATES: usize = 2;

// the encoder `wf-recorder` is given for each kind (`ffmpeg -encoders`)
const VAAPI: &str = "h264_vaapi";
const NVENC: &str = "h264_nvenc";
const SOFTWARE: &str = "libx264";

/*
 * what the VA-API encoder is fed: the screen's RGB converted on the GPU to NV12 in the matrix and
 * range every player assumes for H.264 of this size, BT.709 limited. `wf-recorder`'s own default
 * converts to full range with the encoder's default matrix (BT.601) and no tag, which players
 * then read as BT.709 and show with shifted colors. The `out_color_*` options only describe the
 * frames to the filter graph: no encoder reads them. What the file says is the tag `COLOR` puts on
 * the encoder
 */
pub const VAAPI_FILTER: &str = "scale_vaapi=format=nv12:out_range=tv:out_color_matrix=bt709:out_color_primaries=bt709:out_color_transfer=bt709";

/*
 * the options of the encoder that make its H.264 stream say BT.709, limited range (the video
 * usability information: primaries, transfer, matrix and range), the same for all three. Checked
 * with libavcodec given them through the option dictionary `wf-recorder -p` fills, and the stream's
 * headers read back; not yet through a live capture. Without them h264_vaapi writes no tag at all,
 * and a player takes a picture under 720 lines as BT.601
 */
const COLOR: [&str; 4] = [
    "colorspace=bt709",
    "color_primaries=bt709",
    "color_trc=bt709",
    "color_range=tv",
];

// the same for an encoder that takes system memory
/*
 * a window's frames from the cast to system memory, for the encoders that cannot take them from the
 * cast. niri casts DMA-BUF only; `glupload` takes that (and system memory, when a niri offers it)
 * and the caps with no memory feature keep what `gldownload` hands on in system memory
 */
const DOWNLOAD: [&str; 6] = ["glupload", "!", "gldownload", "!", "video/x-raw", "!"];

const SOFTWARE_FILTER: &str = "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p";

/*
 * the constant quantizer `quality` is for `kind`, or for software the constant rate factor, which an
 * encoder keeps whatever the picture does: a still screen costs nothing and a busy one what it
 * needs. Measured on a 1080p test pattern of text over moving bars (ADR 0116, Update): a quality
 * step changes the size 25 to 30%, below 20 it grows fast for little to see, above 24 text starts
 * to smear. NVENC's and VA-API's scales differ little; x264's crf 19 is about their 20
 */
pub fn quantizer(quality: Quality, kind: Kind) -> u32 {
    match (kind, quality) {
        (Kind::Software, Quality::Low) => 27,
        (Kind::Software, Quality::Medium) => 23,
        (Kind::Software, Quality::High) => 19,
        (Kind::Software, Quality::Max) => 15,
        (_, Quality::Low) => 26,
        (_, Quality::Medium) => 23,
        (_, Quality::High) => 20,
        (_, Quality::Max) => 16,
    }
}

// the way an encoder is run
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    // on an Intel or AMD GPU through VA-API
    Vaapi,

    // on an NVIDIA GPU
    Nvenc,

    // libx264, on the CPU
    Software,
}

impl Kind {
    // what `wf-recorder -c` is given
    pub fn codec(self) -> &'static str {
        match self {
            Kind::Vaapi => VAAPI,
            Kind::Nvenc => NVENC,
            Kind::Software => SOFTWARE,
        }
    }
}

// an encoder chosen: how, and for VA-API on which render node
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoder {
    pub kind: Kind,

    // `/dev/dri/renderD128`, for VA-API
    pub node: Option<String>,

    // whether GStreamer names this node's encoder `vah264enc`: it does for the first it finds, and
    // `varenderD129h264enc` and the like for the rest
    pub first: bool,
}

/*
 * GStreamer's element for a VA-API stage on a node: `vah264enc` on the first, and
 * `varenderD129h264enc` and the like on the rest, with the post-processor likewise. The stages of
 * one chain must be the same node's, or the frames cross displays
 */
fn va_element(node: Option<&str>, first: bool, stage: &str) -> String {
    match node {
        Some(node) if !first => {
            let name = node.rsplit('/').next().unwrap_or(node);

            format!("va{name}{stage}")
        }
        _ => format!("va{stage}"),
    }
}

// how a window's frames get from niri's cast to system memory, for the encoders that take them there
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Download {
    // through EGL: `glupload ! gldownload`. It needs no VA-API driver, and cannot import what Mesa's
    // GL refuses, like Intel's compressed tiled buffers (`EGL_BAD_MATCH`), which niri casts
    #[default]
    Gl,

    // through the VA-API post-processor of a render node, which imports those
    Va {
        node: Option<String>,
        first: bool,
    },
}

impl fmt::Display for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Download::Gl => f.write_str("EGL (glupload)"),
            Download::Va {
                node: Some(node), ..
            } => write!(f, "VA-API on {node} (vapostproc)"),
            Download::Va { node: None, .. } => f.write_str("VA-API (vapostproc)"),
        }
    }
}

impl Encoder {
    pub fn software() -> Encoder {
        Encoder {
            kind: Kind::Software,
            node: None,
            first: true,
        }
    }

    // GStreamer's element for a VA-API stage on this node, see `va_element`
    fn va(&self, stage: &str) -> String {
        va_element(self.node.as_deref(), self.first, stage)
    }

    // GStreamer's element for it
    pub fn element(&self) -> String {
        match self.kind {
            Kind::Vaapi => self.va("h264enc"),
            Kind::Nvenc => String::from("nvh264enc"),
            Kind::Software => String::from("x264enc"),
        }
    }
}

impl fmt::Display for Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.node {
            Some(node) => write!(f, "{} on {node}", self.kind.codec()),
            None => f.write_str(self.kind.codec()),
        }
    }
}

// an encoder with how well to encode and how many threads a software one may use
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoding {
    pub encoder: Encoder,
    pub quality: Quality,
    pub threads: usize,

    // for a window, how NVENC and x264 get the cast's frames; VA-API takes them as they are
    pub download: Download,
}

impl Encoding {
    fn rate_control(&self) -> Vec<(&'static str, String)> {
        let kind = self.encoder.kind;
        let qp = quantizer(self.quality, kind);

        match kind {
            Kind::Vaapi => vec![("rc_mode", String::from("CQP")), ("qp", qp.to_string())],
            Kind::Nvenc => vec![("rc", String::from("constqp")), ("qp", qp.to_string())],
            Kind::Software => vec![
                ("preset", String::from("veryfast")),
                ("crf", qp.to_string()),
                ("threads", self.threads.to_string()),
            ],
        }
    }

    /*
     * what `wf-recorder` is told of it, ahead of the source. `-p` sets an option of the encoder,
     * `-F` replaces the filter between the capture and the encoder, `-x` the pixel format. NVENC and
     * x264 take frames in system memory, so `--no-dmabuf`
     */
    pub fn recorder(&self) -> Vec<String> {
        let kind = self.encoder.kind;
        let mut arguments = vec![String::from("-c"), String::from(kind.codec())];

        for (name, value) in self.rate_control() {
            arguments.extend([String::from("-p"), format!("{name}={value}")]);
        }

        COLOR
            .into_iter()
            .for_each(|option| arguments.extend([String::from("-p"), String::from(option)]));

        match (kind, &self.encoder.node) {
            (Kind::Vaapi, node) => {
                arguments.extend(node.iter().flat_map(|node| ["-d", node]).map(String::from));
                arguments.extend([String::from("-F"), String::from(VAAPI_FILTER)]);
            }
            _ => arguments
                .extend(["--no-dmabuf", "-x", "yuv420p", "-F", SOFTWARE_FILTER].map(String::from)),
        }

        arguments
    }

    /*
     * the GStreamer elements of a window's pipeline from the first converter to the encoder, the
     * `!` between them included, at `size`. `add-borders` keeps the aspect, so a window that is
     * resized stays whole in the frame, provided the cast is held to square pixels
     * (`plan::encoder`; else `vapostproc` cannot calculate the borders and stretches the picture)
     * and the caps here fix them on the output too. The caps name BT.709, the matrix the picture
     * is converted with, which the muxer then tags. niri's cast offers DMA-BUF only (a mandatory modifier list,
     * Intel's compressed tiled ones among them), and `pipewiresrc` offers a format only if the
     * element after it takes it: `vapostproc` does, a CPU converter does not (`no more input
     * formats`, not-negotiated). So NVENC and x264 get the frames in system memory from the VA-API
     * post-processor of a node that works (it converts and scales on the way), or else through EGL
     * (`Download::Gl`), which Mesa refuses for the compressed tiled ones
     */
    pub fn chain(&self, size: (u32, u32)) -> Vec<String> {
        let (width, height) = size;
        let kind = self.encoder.kind;
        let qp = quantizer(self.quality, kind);
        let sized = |memory: &str, format: &str| {
            format!(
                "video/x-raw{memory},width={width},height={height}{format},colorimetry=bt709,pixel-aspect-ratio=1/1"
            )
        };
        // the frames in system memory, in the format the encoder takes, and the encoder
        let on_cpu = |format: &str, encoder: Vec<String>| -> Vec<String> {
            let frames = sized("", &format!(",format={format}"));
            let converted: Vec<String> = match &self.download {
                Download::Va { node, first } => {
                    vec![va_element(node.as_deref(), *first, "postproc")]
                }
                Download::Gl => DOWNLOAD
                    .iter()
                    .chain(&["videoconvertscale"])
                    .map(|word| String::from(*word))
                    .collect(),
            };

            converted
                .into_iter()
                .chain([String::from("add-borders=true"), String::from("!"), frames])
                .chain([String::from("!")])
                .chain(encoder)
                .collect()
        };
        let mut chain: Vec<String> = match kind {
            Kind::Vaapi => vec![
                self.encoder.va("postproc"),
                String::from("add-borders=true"),
                String::from("!"),
                sized("(memory:VAMemory)", ""),
                String::from("!"),
                self.encoder.element(),
                String::from("rate-control=cqp"),
                format!("qpi={qp}"),
                format!("qpp={qp}"),
                format!("qpb={qp}"),
            ],
            Kind::Nvenc => on_cpu(
                "NV12",
                vec![
                    self.encoder.element(),
                    String::from("rc-mode=constqp"),
                    format!("qp-const-i={qp}"),
                    format!("qp-const-p={qp}"),
                    format!("qp-const-b={qp}"),
                ],
            ),
            Kind::Software => on_cpu(
                "I420",
                vec![
                    self.encoder.element(),
                    String::from("pass=quant"),
                    format!("quantizer={qp}"),
                    String::from("speed-preset=veryfast"),
                    format!("threads={}", self.threads),
                ],
            ),
        };

        chain.shrink_to_fit();
        chain
    }

    // the `ffmpeg` arguments that encode one frame of black the way `recorder` would: a real encode
    pub fn trial(&self) -> Vec<String> {
        let kind = self.encoder.kind;
        let source = "color=c=black:s=256x144:r=30:d=0.2,format=bgr0";
        let mut arguments: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
            .map(String::from)
            .to_vec();

        match (kind, &self.encoder.node) {
            (Kind::Vaapi, Some(node)) => arguments.extend(
                ["-init_hw_device", &format!("vaapi=va:{node}")]
                    .map(String::from)
                    .into_iter()
                    .chain(["-filter_hw_device", "va"].map(String::from)),
            ),
            (Kind::Vaapi, None) => arguments.extend(
                ["-init_hw_device", "vaapi=va", "-filter_hw_device", "va"].map(String::from),
            ),
            _ => {}
        }

        arguments.extend(["-f", "lavfi", "-i", source].map(String::from));
        arguments.extend([
            String::from("-vf"),
            match kind {
                Kind::Vaapi => format!("hwupload,{VAAPI_FILTER}"),
                _ => String::from(SOFTWARE_FILTER),
            },
        ]);
        arguments.extend(["-c:v", kind.codec()].map(String::from));
        for (name, value) in self.rate_control() {
            arguments.extend([format!("-{name}"), value]);
        }
        arguments.extend(["-frames:v", "1", "-f", "null", "-"].map(String::from));
        arguments
    }

    // the `gst-launch-1.0` arguments that encode two frames of a test pattern through `chain`, to
    // nothing
    pub fn gst_trial(&self) -> Vec<String> {
        let mut arguments: Vec<String> = [
            "-q",
            "videotestsrc",
            "num-buffers=2",
            "!",
            "video/x-raw,width=256,height=144,framerate=30/1,format=BGRx",
            "!",
        ]
        .map(String::from)
        .to_vec();

        arguments.extend(self.chain((256, 144)));
        arguments.extend(["!", "fakesink"].map(String::from));
        arguments
    }
}

// who made a render node's GPU, from the PCI vendor id in `/sys/class/drm/<node>/device/vendor`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Intel,
    Amd,
    Nvidia,
    Other,
}

impl Vendor {
    pub fn read(text: &str) -> Vendor {
        match text.trim().to_ascii_lowercase().as_str() {
            "0x8086" => Vendor::Intel,
            "0x1002" | "0x1022" => Vendor::Amd,
            "0x10de" => Vendor::Nvidia,
            _ => Vendor::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Vendor::Intel => "Intel",
            Vendor::Amd => "AMD",
            Vendor::Nvidia => "NVIDIA",
            Vendor::Other => "unknown vendor",
        }
    }
}

// a DRM render node
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    // `/dev/dri/renderD128`
    pub path: String,
    pub vendor: Vendor,

    // the kernel driver bound to it, like `i915`
    pub driver: String,

    // the GPU the firmware boots on, which drives the primary display on a hybrid machine
    pub primary: bool,
}

impl Node {
    fn said(&self) -> String {
        match self.driver.is_empty() {
            true => format!("{}, {}", self.path, self.vendor.name()),
            false => format!("{}, {} {}", self.path, self.vendor.name(), self.driver),
        }
    }
}

// the nodes in the order a number sorts: renderD128 before renderD129, renderD1000 after
fn order(path: &str) -> (usize, &str) {
    (path.len(), path)
}

// whether GStreamer names a node's elements `vah264enc`: the lowest numbered of the GPUs that are
// not NVIDIA is its first
/*
 * GStreamer's elements registered by plugin `va` (`gst-inspect-1.0 va`), which names elements per
 * render node when more than one GPU has VA-API (e.g. `varenderD129h264enc` / `varenderD129postproc`)
 * or the default `vah264enc` / `vapostproc` on the first. Parsing what GStreamer has replaces
 * guessing from node names
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Va {
    elements: Vec<String>,
}

impl Va {
    pub fn read(text: &str) -> Self {
        let elements = text
            .lines()
            .filter_map(|line| {
                let token = line.split_whitespace().next()?;
                let name = token.trim_end_matches(':');
                if name.starts_with("va") {
                    Some(name.to_owned())
                } else {
                    None
                }
            })
            .collect();

        Self { elements }
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn has(&self, element: &str) -> bool {
        self.elements.iter().any(|elem| elem == element)
    }

    /*
     * whether `node` uses the first/default elements `vah264enc` / `vapostproc` rather than
     * node-specific ones like `va<node>h264enc`. GStreamer's inspect output decides when present;
     * an empty inspection falls back to whether this is the lowest numbered non-NVIDIA node
     */
    pub fn is_first(&self, nodes: &[Node], node: &Node) -> bool {
        let name = node.path.rsplit('/').next().unwrap_or(&node.path);
        let node_specific = format!("va{name}h264enc");

        if self.has(&node_specific) {
            false
        } else if self.has("vah264enc") {
            true
        } else {
            is_first(nodes, node)
        }
    }
}

// whether `node` is the first non-NVIDIA render node, as a fallback when GStreamer was not inspected
pub fn is_first(nodes: &[Node], node: &Node) -> bool {
    nodes
        .iter()
        .filter(|node| node.vendor != Vendor::Nvidia)
        .map(|node| order(&node.path))
        .min()
        .is_none_or(|lowest| lowest == order(&node.path))
}

/*
 * the render nodes VA-API may encode on, in the order to try. A node the user names is the only
 * one, found among `nodes` or trusted as given. Else those of Intel and AMD (the NVIDIA driver's
 * VA-API is for decoding only), the one that drives the primary display first
 */
pub fn candidates(nodes: &[Node], device: Option<&str>) -> Vec<Node> {
    if let Some(device) = device {
        return vec![
            nodes
                .iter()
                .find(|node| node.path == device)
                .cloned()
                .unwrap_or_else(|| Node {
                    path: device.to_owned(),
                    vendor: Vendor::Other,
                    driver: String::new(),
                    primary: false,
                }),
        ];
    }

    let mut found: Vec<Node> = nodes
        .iter()
        .filter(|node| matches!(node.vendor, Vendor::Intel | Vendor::Amd))
        .cloned()
        .collect();

    found.sort_by(|a, b| {
        b.primary
            .cmp(&a.primary)
            .then_with(|| order(&a.path).cmp(&order(&b.path)))
    });
    found
}

/*
 * the ways to get a window's frames to system memory, in the order to try: the VA-API
 * post-processor of each node of `candidates`, then EGL. With `no_va`, which a live cast sets after
 * a VA-API download failed on it, EGL only, so no trial brings the failed download back
 */
pub fn downloads(nodes: &[Node], device: Option<&str>, no_va: bool, va: &Va) -> Vec<Download> {
    let candidates = match no_va {
        true => Vec::new(),
        false => candidates(nodes, device),
    };

    candidates
        .into_iter()
        .map(|node| Download::Va {
            first: va.is_first(nodes, &node),
            node: Some(node.path),
        })
        .chain([Download::Gl])
        .collect()
}

// what a probe found of one way to encode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probed {
    Works,

    // it ran and failed, with what it said
    Fails(String),

    // it could not be tried, like with no `ffmpeg` to try it with
    Unknown(String),
}

// tries the ways to encode for real, one frame each
pub trait Probe {
    fn vaapi(&mut self, node: &Node, first: bool, timeout: Duration) -> Probed;
    fn nvenc(&mut self, timeout: Duration) -> Probed;
    fn software(&mut self, timeout: Duration) -> Probed;
}

// what was chosen, and why, for `doctor` and the failure that says what it tried
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    // none when nothing encodes
    pub encoder: Option<Encoder>,

    // each way tried or skipped, and what the probe said
    pub notes: Vec<String>,
}

/*
 * the encoder to record with. `Auto` tries VA-API on each candidate node, then NVENC, then
 * software, and takes the first that encodes a frame: NVENC is not tried while VA-API works, so
 * the NVIDIA GPU of a hybrid machine stays asleep. A named encoder that does not work gives way to
 * software, said in the notes. A probe that could not run (`Unknown`) lets VA-API and software
 * through, which a failure then falls back from at run time, and NVENC only when it is named
 */
#[allow(dead_code)]
pub fn choose(
    preference: Preference,
    device: Option<&str>,
    nodes: &[Node],
    probe: &mut impl Probe,
) -> Choice {
    choose_within(
        preference,
        device,
        nodes,
        probe,
        Instant::now() + PROBE_BUDGET,
        &Va::default(),
    )
}

pub fn choose_within(
    preference: Preference,
    device: Option<&str>,
    nodes: &[Node],
    probe: &mut impl Probe,
    deadline: Instant,
    va: &Va,
) -> Choice {
    let mut notes = Vec::new();
    let kinds: &[Kind] = match preference {
        Preference::Auto => &[Kind::Vaapi, Kind::Nvenc, Kind::Software],
        Preference::Vaapi => &[Kind::Vaapi, Kind::Software],
        Preference::Nvenc => &[Kind::Nvenc, Kind::Software],
        Preference::Software => &[Kind::Software],
    };
    let named = preference != Preference::Auto;

    for &kind in kinds {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            notes.push(String::from(
                "probe budget exceeded; falling back to software",
            ));
            return Choice {
                encoder: Some(Encoder::software()),
                notes,
            };
        }

        let found = match kind {
            Kind::Vaapi => {
                let tried = candidates(nodes, device);

                if tried.is_empty() {
                    notes.push(String::from(
                        "VA-API: no Intel or AMD render node under /dev/dri",
                    ));
                }

                tried.iter().take(MAX_VAAPI_CANDIDATES).find_map(|node| {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        notes.push(format!(
                            "VA-API on {} not tried: probe budget exceeded",
                            node.said()
                        ));
                        return None;
                    }
                    let timeout = TRIAL.min(remaining);
                    let first = va.is_first(nodes, node);

                    match probe.vaapi(node, first, timeout) {
                        Probed::Works => {
                            notes.push(format!("VA-API works on {}", node.said()));
                            Some(Encoder {
                                kind,
                                node: Some(node.path.clone()),
                                first,
                            })
                        }
                        Probed::Unknown(why) => {
                            notes.push(format!("VA-API on {} not tried: {why}", node.said()));
                            Some(Encoder {
                                kind,
                                node: Some(node.path.clone()),
                                first,
                            })
                        }
                        Probed::Fails(why) => {
                            notes.push(format!("VA-API fails on {}: {why}", node.said()));
                            None
                        }
                    }
                })
            }
            Kind::Nvenc => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    notes.push(String::from("NVENC not tried: probe budget exceeded"));
                    None
                } else {
                    let timeout = TRIAL.min(remaining);
                    match probe.nvenc(timeout) {
                        Probed::Works => {
                            notes.push(String::from("NVENC works"));
                            Some(Encoder {
                                kind,
                                node: None,
                                first: true,
                            })
                        }
                        Probed::Unknown(why) if named => {
                            notes.push(format!("NVENC not tried: {why}"));
                            Some(Encoder {
                                kind,
                                node: None,
                                first: true,
                            })
                        }
                        Probed::Unknown(why) => {
                            notes.push(format!("NVENC not tried: {why}"));
                            None
                        }
                        Probed::Fails(why) => {
                            notes.push(format!("NVENC fails: {why}"));
                            None
                        }
                    }
                }
            }
            Kind::Software => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    notes.push(String::from(
                        "software x264 not tried: probe budget exceeded",
                    ));
                    Some(Encoder::software())
                } else {
                    let timeout = TRIAL.min(remaining);
                    match probe.software(timeout) {
                        Probed::Works => {
                            notes.push(String::from("software x264 works"));
                            Some(Encoder::software())
                        }
                        Probed::Unknown(why) => {
                            notes.push(format!("software x264 not tried: {why}"));
                            Some(Encoder::software())
                        }
                        Probed::Fails(why) => {
                            notes.push(format!("software x264 fails: {why}"));
                            None
                        }
                    }
                }
            }
        };

        if found.is_some() {
            return Choice {
                encoder: found,
                notes,
            };
        }
    }

    Choice {
        encoder: None,
        notes,
    }
}

/*
 * what to install for a GPU encoder that was tried and failed, as the README's table names the
 * packages: the VA-API driver of an Intel or AMD GPU, NVENC's of an NVIDIA one. Read from the
 * choice's notes, so an encoder that was not tried, like VA-API when NVENC is named, is not blamed
 */
pub fn hints(nodes: &[Node], choice: &Choice) -> Vec<String> {
    let failed = |start: &str| choice.notes.iter().any(|note| note.starts_with(start));
    let has = |vendor: Vendor| nodes.iter().any(|node| node.vendor == vendor);
    let mut hints = Vec::new();

    // the note of a VA-API failure names its node's vendor
    let blamed = |vendor: Vendor| {
        choice
            .notes
            .iter()
            .any(|note| note.starts_with("VA-API fails") && note.contains(vendor.name()))
    };

    if has(Vendor::Intel) && blamed(Vendor::Intel) {
        hints.push(String::from(
            "intel-media-driver (VA-API for the Intel GPU)",
        ));
    }

    if has(Vendor::Amd) && blamed(Vendor::Amd) {
        hints.push(String::from("mesa (VA-API for the AMD GPU)"));
    }

    if failed("NVENC fails") && has(Vendor::Nvidia) {
        hints.push(String::from("nvidia-utils (NVENC for the NVIDIA GPU)"));
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str, vendor: Vendor, primary: bool) -> Node {
        Node {
            path: path.to_owned(),
            vendor,
            driver: String::new(),
            primary,
        }
    }

    // what a hybrid laptop has: Intel on the first node, NVIDIA on the second
    fn hybrid() -> Vec<Node> {
        vec![
            node("/dev/dri/renderD129", Vendor::Nvidia, false),
            node("/dev/dri/renderD128", Vendor::Intel, true),
        ]
    }

    // answers a probe from a table and records what was asked, in order
    #[derive(Default)]
    struct Asked {
        works: Vec<&'static str>,
        asked: Vec<String>,
    }

    impl Asked {
        fn works(works: &[&'static str]) -> Asked {
            Asked {
                works: works.to_vec(),
                asked: Vec::new(),
            }
        }

        fn answer(&mut self, name: String) -> Probed {
            let works = self.works.iter().any(|works| name.starts_with(works));

            self.asked.push(name);

            match works {
                true => Probed::Works,
                false => Probed::Fails(String::from("no")),
            }
        }
    }

    impl Probe for Asked {
        fn vaapi(&mut self, node: &Node, _first: bool, _timeout: Duration) -> Probed {
            self.answer(format!("vaapi {}", node.path))
        }

        fn nvenc(&mut self, _timeout: Duration) -> Probed {
            self.answer(String::from("nvenc"))
        }

        fn software(&mut self, _timeout: Duration) -> Probed {
            self.answer(String::from("software"))
        }
    }

    fn encoder(choice: &Choice) -> String {
        choice
            .encoder
            .as_ref()
            .map_or_else(|| String::from("none"), Encoder::to_string)
    }

    #[test]
    fn a_vendor_is_read_from_its_pci_id() {
        assert_eq!(Vendor::read("0x8086\n"), Vendor::Intel);
        assert_eq!(Vendor::read("0x1002"), Vendor::Amd);
        assert_eq!(Vendor::read("0x10DE\n"), Vendor::Nvidia);
        assert_eq!(Vendor::read("0x1af4"), Vendor::Other);
        assert_eq!(Vendor::read(""), Vendor::Other);
    }

    // never the NVIDIA node, the primary GPU first, and a named node the only one
    #[test]
    fn vaapi_tries_intel_and_amd_nodes_and_never_nvidia() {
        let mut nodes = hybrid();
        nodes.push(node("/dev/dri/renderD130", Vendor::Amd, false));

        let tried: Vec<String> = candidates(&nodes, None)
            .into_iter()
            .map(|node| node.path)
            .collect();

        assert_eq!(tried, ["/dev/dri/renderD128", "/dev/dri/renderD130"]);

        // the AMD GPU drives the display: it comes first
        nodes[2].primary = true;
        nodes[1].primary = false;

        assert_eq!(candidates(&nodes, None)[0].path, "/dev/dri/renderD130");

        // a node the user names is the only one, listed or not
        let named = candidates(&nodes, Some("/dev/dri/renderD129"));

        assert_eq!(named.len(), 1);
        assert_eq!(named[0].vendor, Vendor::Nvidia);
        assert_eq!(
            candidates(&nodes, Some("/dev/dri/renderD200"))[0].path,
            "/dev/dri/renderD200"
        );
    }

    // a hybrid laptop's NVIDIA GPU is not woken while VA-API on the Intel one works
    #[test]
    fn auto_prefers_vaapi_and_does_not_wake_the_nvidia_gpu() {
        let mut probe = Asked::works(&["vaapi", "nvenc", "software"]);
        let choice = choose(Preference::Auto, None, &hybrid(), &mut probe);

        assert_eq!(encoder(&choice), "h264_vaapi on /dev/dri/renderD128");
        assert_eq!(probe.asked, ["vaapi /dev/dri/renderD128"]);
    }

    #[test]
    fn auto_goes_on_to_nvenc_then_software_as_each_fails() {
        let mut probe = Asked::works(&["nvenc", "software"]);
        let choice = choose(Preference::Auto, None, &hybrid(), &mut probe);

        assert_eq!(encoder(&choice), "h264_nvenc");
        assert_eq!(probe.asked, ["vaapi /dev/dri/renderD128", "nvenc"]);

        let mut probe = Asked::works(&["software"]);
        let choice = choose(Preference::Auto, None, &hybrid(), &mut probe);

        assert_eq!(encoder(&choice), "libx264");
        assert!(
            choice
                .notes
                .iter()
                .any(|note| note.contains("VA-API fails"))
        );
        assert!(choice.notes.iter().any(|note| note.contains("NVENC fails")));

        // a machine with no GPU node at all
        let mut probe = Asked::works(&["software"]);

        assert_eq!(
            encoder(&choose(Preference::Auto, None, &[], &mut probe)),
            "libx264"
        );
        assert_eq!(probe.asked, ["nvenc", "software"]);
    }

    // a named encoder that does not work is said so and gives way to software; nothing at all
    // encoding is no encoder, not a guess
    #[test]
    fn a_named_encoder_that_fails_falls_back_to_software_and_says_so() {
        let mut probe = Asked::works(&["software"]);
        let choice = choose(Preference::Nvenc, None, &hybrid(), &mut probe);

        assert_eq!(encoder(&choice), "libx264");
        assert_eq!(probe.asked, ["nvenc", "software"]);
        assert!(choice.notes[0].starts_with("NVENC fails"));

        let mut probe = Asked::works(&[]);
        let choice = choose(Preference::Software, None, &hybrid(), &mut probe);

        assert_eq!(choice.encoder, None);
        assert_eq!(probe.asked, ["software"]);

        // `vaapi` does not try the NVIDIA node
        let mut probe = Asked::works(&["nvenc"]);

        assert_eq!(
            encoder(&choose(Preference::Vaapi, None, &hybrid(), &mut probe)),
            "none"
        );
        assert_eq!(probe.asked, ["vaapi /dev/dri/renderD128", "software"]);
    }

    // without `ffmpeg` to probe with, VA-API and software are let through to fall back at run time,
    // NVENC only when asked for
    #[test]
    fn an_unprobed_way_to_encode_is_used_only_where_a_failure_can_fall_back() {
        struct Blind;

        impl Probe for Blind {
            fn vaapi(&mut self, _: &Node, _: bool, _: Duration) -> Probed {
                Probed::Unknown(String::from("no ffmpeg"))
            }

            fn nvenc(&mut self, _: Duration) -> Probed {
                Probed::Unknown(String::from("no ffmpeg"))
            }

            fn software(&mut self, _: Duration) -> Probed {
                Probed::Unknown(String::from("no ffmpeg"))
            }
        }

        assert_eq!(
            encoder(&choose(Preference::Auto, None, &hybrid(), &mut Blind)),
            "h264_vaapi on /dev/dri/renderD128"
        );
        assert_eq!(
            encoder(&choose(Preference::Auto, None, &[], &mut Blind)),
            "libx264"
        );
        assert_eq!(
            encoder(&choose(Preference::Nvenc, None, &hybrid(), &mut Blind)),
            "h264_nvenc"
        );
    }

    // doctor names a package only for a GPU encoder that was tried and failed
    #[test]
    fn a_failed_gpu_encoder_is_named_by_its_package() {
        let nodes = hybrid();
        let hinted = |preference, works: &[&'static str]| {
            hints(
                &nodes,
                &choose(preference, None, &nodes, &mut Asked::works(works)),
            )
        };

        assert_eq!(
            hinted(Preference::Auto, &["software"]),
            [
                "intel-media-driver (VA-API for the Intel GPU)",
                "nvidia-utils (NVENC for the NVIDIA GPU)"
            ]
        );

        // VA-API works: nothing is missing, and NVENC was never tried
        assert!(hinted(Preference::Auto, &["vaapi"]).is_empty());

        // NVENC named and working: VA-API is not blamed for not being tried
        assert!(hinted(Preference::Nvenc, &["nvenc"]).is_empty());
    }

    #[test]
    fn sequential_vaapi_probes_are_bounded() {
        let nodes = vec![
            node("/dev/dri/renderD128", Vendor::Intel, true),
            node("/dev/dri/renderD129", Vendor::Amd, false),
            node("/dev/dri/renderD130", Vendor::Intel, false),
            node("/dev/dri/renderD131", Vendor::Amd, false),
        ];
        let mut probe = Asked::works(&["software"]);
        let _ = choose(Preference::Auto, None, &nodes, &mut probe);

        let vaapi_asked = probe.asked.iter().filter(|s| s.contains("vaapi")).count();
        assert_eq!(vaapi_asked, MAX_VAAPI_CANDIDATES);
    }

    #[test]
    fn probe_budget_exhaustion_falls_back_to_software() {
        let nodes = hybrid();
        let mut probe = Asked::works(&["software"]);
        let past = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        let choice = choose_within(
            Preference::Auto,
            None,
            &nodes,
            &mut probe,
            past,
            &Va::default(),
        );
        assert_eq!(encoder(&choice), "libx264");
        assert!(
            probe.asked.is_empty(),
            "no probes should run when budget is exhausted"
        );
        assert!(
            choice
                .notes
                .iter()
                .any(|note| note.contains("probe budget exceeded"))
        );
    }

    fn encoding(kind: Kind, quality: Quality) -> Encoding {
        Encoding {
            encoder: Encoder {
                kind,
                node: (kind == Kind::Vaapi).then(|| String::from("/dev/dri/renderD128")),
                first: true,
            },
            quality,
            threads: 3,
            download: Download::Gl,
        }
    }

    // a higher quality is never a higher quantizer, for any encoder
    #[test]
    fn a_higher_quality_is_a_lower_quantizer() {
        for kind in [Kind::Vaapi, Kind::Nvenc, Kind::Software] {
            let qp: Vec<u32> = [Quality::Low, Quality::Medium, Quality::High, Quality::Max]
                .map(|quality| quantizer(quality, kind))
                .to_vec();

            assert!(qp.windows(2).all(|pair| pair[0] > pair[1]), "{kind:?}");
        }
    }

    // frames cross from the post-processor to the encoder, so both must be the same GPU's: a
    // second render node has its own display, and `vah264enc` after `vapostproc` is the first's
    #[test]
    fn a_windows_vaapi_stages_are_of_one_node() {
        let on = |node: &str, first| {
            Encoding {
                encoder: Encoder {
                    kind: Kind::Vaapi,
                    node: Some(node.into()),
                    first,
                },
                quality: Quality::High,
                threads: 3,
                download: Download::Gl,
            }
            .chain((800, 600))
            .join(" ")
        };

        let first = on("/dev/dri/renderD128", true);
        let second = on("/dev/dri/renderD129", false);

        assert!(first.starts_with("vapostproc ") && first.contains("! vah264enc "));
        assert!(
            second.starts_with("varenderD129postproc ")
                && second.contains("! varenderD129h264enc ")
        );
    }

    #[test]
    fn a_live_failure_of_the_va_download_leaves_only_egl_to_try() {
        let nodes = hybrid();

        assert!(
            downloads(&nodes, None, false, &Va::default())
                .iter()
                .any(|download| matches!(download, Download::Va { .. }))
        );
        assert_eq!(
            downloads(&nodes, None, true, &Va::default()),
            vec![Download::Gl]
        );
        assert_eq!(
            downloads(&nodes, Some("/dev/dri/renderD128"), true, &Va::default()),
            vec![Download::Gl]
        );
        assert_eq!(
            downloads(&[], None, false, &Va::default()),
            vec![Download::Gl]
        );
    }

    // niri casts DMA-BUF only and `pipewiresrc` offers a format only if the next element takes it: a
    // CPU converter right after it fails to negotiate, and EGL's `glupload` cannot import Intel's
    // compressed tiled buffers (both seen live), so the CPU encoders' frames come from the VA-API
    // post-processor where one works, else through EGL; VA-API's own chain has neither
    #[test]
    fn a_windows_cpu_encoders_download_the_casts_dmabuf_before_converting() {
        for (kind, encoder) in [(Kind::Nvenc, "nvh264enc"), (Kind::Software, "x264enc")] {
            let mut encoding = encoding(kind, Quality::High);
            let gl = encoding.chain((800, 600)).join(" ");

            assert!(
                gl.starts_with("glupload ! gldownload ! video/x-raw ! videoconvertscale "),
                "{gl}"
            );
            assert!(gl.contains(&format!("! {encoder} ")), "{gl}");

            encoding.download = Download::Va {
                node: Some(String::from("/dev/dri/renderD129")),
                first: false,
            };

            let va = encoding.chain((800, 600)).join(" ");

            assert!(
                va.starts_with("varenderD129postproc add-borders=true ! video/x-raw,"),
                "{va}"
            );
            assert!(va.contains(&format!("! {encoder} ")), "{va}");
            assert!(!va.contains("gl") && !va.contains("videoconvert"), "{va}");
        }

        let vaapi = encoding(Kind::Vaapi, Quality::High)
            .chain((800, 600))
            .join(" ");

        assert!(
            vaapi.starts_with("vapostproc ") && !vaapi.contains("gl"),
            "{vaapi}"
        );
    }

    // what is probed is what is run: the trial of an encoder carries the filter and codec of its
    // recorder's arguments, and the same rate control
    #[test]
    fn a_trial_encodes_with_the_recorders_codec_and_filter() {
        let vaapi = encoding(Kind::Vaapi, Quality::High).trial().join(" ");

        assert!(vaapi.contains("-init_hw_device vaapi=va:/dev/dri/renderD128"));
        assert!(vaapi.contains(&format!("hwupload,{VAAPI_FILTER}")));
        assert!(vaapi.contains("-c:v h264_vaapi"));
        assert!(vaapi.contains("-rc_mode CQP -qp 20"));

        let software = encoding(Kind::Software, Quality::High).trial().join(" ");

        assert!(software.contains(SOFTWARE_FILTER));
        assert!(software.contains("-c:v libx264"));
        assert!(software.contains("-preset veryfast -crf 19 -threads 3"));
        assert!(!software.contains("vaapi"));

        let nvenc = encoding(Kind::Nvenc, Quality::High).trial().join(" ");
        assert!(nvenc.contains("-rc constqp -qp 20"));
    }

    #[test]
    fn gst_inspect_va_output_names_first_and_secondary_node_elements() {
        let fixture = "\
Plugin Details:
  Name                     va
  Description              VA-API codecs plugin

  vah264enc: VA-API H.264 Encoder in Intel(R) Gen Graphics
  vapostproc: VA-API Video Postprocessor in Intel(R) Gen Graphics
  varenderD129h264enc: VA-API H.264 Encoder in AMD Radeon
  varenderD129postproc: VA-API Video Postprocessor in AMD Radeon
";
        let va = Va::read(fixture);
        assert!(va.has("vah264enc"));
        assert!(va.has("vapostproc"));
        assert!(va.has("varenderD129h264enc"));
        assert!(va.has("varenderD129postproc"));

        let intel = node("/dev/dri/renderD128", Vendor::Intel, true);
        let amd = node("/dev/dri/renderD129", Vendor::Amd, false);
        let nodes = vec![intel.clone(), amd.clone()];

        assert!(va.is_first(&nodes, &intel));
        assert!(!va.is_first(&nodes, &amd));
    }
}
