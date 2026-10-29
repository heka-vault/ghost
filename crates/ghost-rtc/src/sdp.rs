//! SDP shaping: what Ghost reads out of the phone's offer before any stack sees it, and
//! what every stack's answer must carry. Deterministic and typed (CLAUDE.md): the offer is
//! untrusted text from the network; this keeps the fields the call needs and drops the
//! rest, and an error names the missing part, never a line of the offer.
//!
//! The phone offers what Pipecat's aiortc peer and react-native-webrtc both produce: one
//! audio m-line with Opus at 48 kHz stereo, one `application` m-line for the `chat` data
//! channel, `a=setup:actpass`, a sha-256 fingerprint, ICE credentials, host candidates
//! only (no ICE servers, ADR 0016). The answer pins the desk's DTLS certificate through
//! `a=fingerprint`, signed by the listener (`docs/phone-protocol.md`).

use std::fmt;

use sha2::{Digest, Sha256};

/// A DTLS fingerprint as SDP writes it: the hash name and upper-case colon-separated hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub algorithm: String,
    pub value: String,
}

impl Fingerprint {
    /// From an `a=fingerprint:` value (`sha-256 AB:CD:...`), the `a=fingerprint:` prefix
    /// optional.
    pub fn parse(text: &str) -> Option<Fingerprint> {
        let text = text.trim();
        let text = text.strip_prefix("a=fingerprint:").unwrap_or(text);
        let mut parts = text.split_whitespace();
        let algorithm = parts.next()?.to_ascii_lowercase();
        let value = parts.next()?;
        if parts.next().is_some() || value.is_empty() {
            return None;
        }
        let bytes: Vec<&str> = value.split(':').collect();
        if bytes
            .iter()
            .any(|b| b.len() != 2 || !b.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return None;
        }
        Some(Fingerprint {
            algorithm,
            value: value.to_ascii_uppercase(),
        })
    }

    /// The sha-256 fingerprint of a DER certificate.
    pub fn from_der(der: &[u8]) -> Fingerprint {
        let digest = Sha256::digest(der);
        Fingerprint::from_bytes("sha-256", &digest)
    }

    pub fn from_bytes(algorithm: &str, bytes: &[u8]) -> Fingerprint {
        let value = bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        Fingerprint {
            algorithm: algorithm.to_ascii_lowercase(),
            value,
        }
    }

    pub fn is_sha256(&self) -> bool {
        self.algorithm == "sha-256"
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.algorithm, self.value)
    }
}

/// `a=setup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    ActPass,
    Active,
    Passive,
}

impl Setup {
    fn parse(s: &str) -> Option<Setup> {
        match s.trim() {
            "actpass" => Some(Setup::ActPass),
            "active" => Some(Setup::Active),
            "passive" => Some(Setup::Passive),
            _ => None,
        }
    }
}

/// A media line's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
}

/// The audio m-line, as far as the call needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioLine {
    pub mid: String,
    /// Opus's payload type in this offer (111 from browsers and aiortc, 96 from some
    /// stacks).
    pub opus_pt: u8,
    pub direction: Direction,
    pub rtcp_mux: bool,
    /// Opus's `a=fmtp` parameters, verbatim (`minptime=10;useinbandfec=1`).
    pub opus_fmtp: Option<String>,
}

/// The data channel's m-line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLine {
    pub mid: String,
    pub sctp_port: Option<u16>,
}

/// The phone's offer, typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub ice_ufrag: String,
    pub ice_pwd: String,
    pub ice_lite: bool,
    pub trickle: bool,
    pub fingerprint: Fingerprint,
    pub setup: Setup,
    pub audio: AudioLine,
    pub data: Option<DataLine>,
    /// The `a=candidate` values, verbatim, without the `a=candidate:` prefix.
    pub candidates: Vec<String>,
    pub bundle: Vec<String>,
}

/// What an answer must carry, checked after each stack produces one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerFacts {
    pub fingerprint: Fingerprint,
    pub setup: Setup,
    pub ice_ufrag: String,
    pub ice_lite: bool,
    pub audio: AudioLine,
    pub has_data: bool,
    pub candidates: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SdpError {
    #[error("not an SDP")]
    NotSdp,
    #[error("no audio m-line")]
    NoAudio,
    #[error("more than one audio m-line")]
    TooManyAudio,
    #[error("no Opus at 48 kHz in the audio m-line")]
    NoOpus,
    #[error("no DTLS fingerprint")]
    NoFingerprint,
    #[error("the DTLS fingerprint is not sha-256")]
    NotSha256,
    #[error("no ICE credentials")]
    NoIce,
    #[error("no a=setup")]
    NoSetup,
    #[error("an offer's setup must be actpass or active")]
    OfferSetup,
    #[error("an answer's setup must be active or passive")]
    AnswerSetup,
    #[error("no a=mid on the audio m-line")]
    NoMid,
    #[error("no rtcp-mux on the audio m-line")]
    NoRtcpMux,
    #[error("the answer's audio is not sendrecv")]
    AnswerNotSendRecv,
    #[error("the answer has no candidate")]
    NoCandidate,
    #[error("the offer has no candidate and does not trickle")]
    NoCandidateNoTrickle,
}

/// One parsed section: the session part or one m-line with its attributes.
#[derive(Default)]
struct Section {
    media: Option<String>,
    fmts: Vec<String>,
    mid: Option<String>,
    ice_ufrag: Option<String>,
    ice_pwd: Option<String>,
    ice_lite: bool,
    trickle: bool,
    fingerprints: Vec<Fingerprint>,
    setup: Option<Setup>,
    direction: Option<Direction>,
    rtcp_mux: bool,
    rtpmap: Vec<(u8, String)>,
    fmtp: Vec<(u8, String)>,
    candidates: Vec<String>,
    bundle: Vec<String>,
    sctp_port: Option<u16>,
}

fn parse_sections(sdp: &str) -> Result<Vec<Section>, SdpError> {
    let mut lines = sdp.lines().map(|l| l.trim_end_matches('\r'));
    if lines.next().map(str::trim) != Some("v=0") {
        return Err(SdpError::NotSdp);
    }
    let mut sections = vec![Section::default()];
    for line in lines {
        let Some((kind, rest)) = line.split_once('=') else {
            continue;
        };
        match kind {
            "m" => {
                let mut parts = rest.split_whitespace();
                let media = parts.next().unwrap_or_default().to_string();
                let _port = parts.next();
                let _proto = parts.next();
                let fmts = parts.map(str::to_string).collect();
                sections.push(Section {
                    media: Some(media),
                    fmts,
                    ..Section::default()
                });
            }
            "a" => {
                let section = sections.last_mut().expect("a session section");
                let (name, value) = match rest.split_once(':') {
                    Some((n, v)) => (n, Some(v)),
                    None => (rest, None),
                };
                match (name, value) {
                    ("mid", Some(v)) => section.mid = Some(v.trim().to_string()),
                    ("ice-ufrag", Some(v)) => section.ice_ufrag = Some(v.trim().to_string()),
                    ("ice-pwd", Some(v)) => section.ice_pwd = Some(v.trim().to_string()),
                    ("ice-lite", _) => section.ice_lite = true,
                    ("ice-options", Some(v)) => {
                        section.trickle |= v.split_whitespace().any(|o| o == "trickle")
                    }
                    ("fingerprint", Some(v)) => section.fingerprints.extend(Fingerprint::parse(v)),
                    ("setup", Some(v)) => section.setup = Setup::parse(v),
                    ("sendrecv", None) => section.direction = Some(Direction::SendRecv),
                    ("sendonly", None) => section.direction = Some(Direction::SendOnly),
                    ("recvonly", None) => section.direction = Some(Direction::RecvOnly),
                    ("inactive", None) => section.direction = Some(Direction::Inactive),
                    ("rtcp-mux", None) => section.rtcp_mux = true,
                    ("rtpmap", Some(v)) => {
                        if let Some((pt, codec)) = v.trim().split_once(' ')
                            && let Ok(pt) = pt.parse::<u8>()
                        {
                            section.rtpmap.push((pt, codec.trim().to_string()));
                        }
                    }
                    ("fmtp", Some(v)) => {
                        if let Some((pt, params)) = v.trim().split_once(' ')
                            && let Ok(pt) = pt.parse::<u8>()
                        {
                            section.fmtp.push((pt, params.trim().to_string()));
                        }
                    }
                    ("candidate", Some(v)) => section.candidates.push(v.trim().to_string()),
                    ("end-of-candidates", None) => {}
                    ("group", Some(v)) => {
                        let mut parts = v.split_whitespace();
                        if parts.next() == Some("BUNDLE") {
                            section.bundle = parts.map(str::to_string).collect();
                        }
                    }
                    ("sctp-port", Some(v)) => section.sctp_port = v.trim().parse().ok(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(sections)
}

/// The Opus payload type of an audio section: `opus/48000/2` (the channel count is what
/// Opus's RTP mapping always says; mono audio still rides it).
fn opus_pt(section: &Section) -> Option<u8> {
    section
        .rtpmap
        .iter()
        .find(|(_, codec)| {
            let lower = codec.to_ascii_lowercase();
            lower.starts_with("opus/48000")
        })
        .map(|(pt, _)| *pt)
}

fn audio_line(session: &Section, section: &Section, is_offer: bool) -> Result<AudioLine, SdpError> {
    let pt = opus_pt(section).ok_or(SdpError::NoOpus)?;
    if !section.fmts.iter().any(|f| f == &pt.to_string()) {
        return Err(SdpError::NoOpus);
    }
    let mid = section.mid.clone().ok_or(SdpError::NoMid)?;
    if !section.rtcp_mux && !session.rtcp_mux {
        return Err(SdpError::NoRtcpMux);
    }
    let direction = section
        .direction
        .or(session.direction)
        .unwrap_or(Direction::SendRecv);
    let _ = is_offer;
    Ok(AudioLine {
        mid,
        opus_pt: pt,
        direction,
        rtcp_mux: true,
        opus_fmtp: section
            .fmtp
            .iter()
            .find(|(p, _)| *p == pt)
            .map(|(_, params)| params.clone()),
    })
}

struct Common {
    ice_ufrag: String,
    ice_pwd: String,
    ice_lite: bool,
    trickle: bool,
    fingerprint: Fingerprint,
    setup: Setup,
    audio: AudioLine,
    data: Option<DataLine>,
    candidates: Vec<String>,
    bundle: Vec<String>,
}

fn parse_common(sdp: &str, is_offer: bool) -> Result<Common, SdpError> {
    let sections = parse_sections(sdp)?;
    let (session, media) = sections.split_first().expect("a session section");
    let audio_sections: Vec<&Section> = media
        .iter()
        .filter(|s| s.media.as_deref() == Some("audio"))
        .collect();
    let audio_section = match audio_sections.as_slice() {
        [] => return Err(SdpError::NoAudio),
        [one] => *one,
        _ => return Err(SdpError::TooManyAudio),
    };
    let audio = audio_line(session, audio_section, is_offer)?;
    let data = media
        .iter()
        .find(|s| s.media.as_deref() == Some("application"))
        .map(|s| DataLine {
            mid: s.mid.clone().unwrap_or_default(),
            sctp_port: s.sctp_port,
        });
    // aiortc offers sha-256, sha-384 and sha-512; the phone pins the sha-256 one.
    let fingerprints = if audio_section.fingerprints.is_empty() {
        &session.fingerprints
    } else {
        &audio_section.fingerprints
    };
    if fingerprints.is_empty() {
        return Err(SdpError::NoFingerprint);
    }
    let fingerprint = fingerprints
        .iter()
        .find(|f| f.is_sha256())
        .cloned()
        .ok_or(SdpError::NotSha256)?;
    let ice_ufrag = audio_section
        .ice_ufrag
        .clone()
        .or_else(|| session.ice_ufrag.clone())
        .ok_or(SdpError::NoIce)?;
    let ice_pwd = audio_section
        .ice_pwd
        .clone()
        .or_else(|| session.ice_pwd.clone())
        .ok_or(SdpError::NoIce)?;
    let setup = audio_section
        .setup
        .or(session.setup)
        .ok_or(SdpError::NoSetup)?;
    let mut candidates = session.candidates.clone();
    for s in media {
        candidates.extend(s.candidates.iter().cloned());
    }
    Ok(Common {
        ice_ufrag,
        ice_pwd,
        ice_lite: session.ice_lite || media.iter().any(|s| s.ice_lite),
        trickle: session.trickle || media.iter().any(|s| s.trickle),
        fingerprint,
        setup,
        audio,
        data,
        candidates,
        bundle: session.bundle.clone(),
    })
}

/// The phone's offer, or why it is not one Ghost answers (`bad_offer`).
pub fn parse_offer(sdp: &str) -> Result<Offer, SdpError> {
    let c = parse_common(sdp, true)?;
    if c.setup == Setup::Passive {
        return Err(SdpError::OfferSetup);
    }
    if c.candidates.is_empty() && !c.trickle {
        return Err(SdpError::NoCandidateNoTrickle);
    }
    Ok(Offer {
        ice_ufrag: c.ice_ufrag,
        ice_pwd: c.ice_pwd,
        ice_lite: c.ice_lite,
        trickle: c.trickle,
        fingerprint: c.fingerprint,
        setup: c.setup,
        audio: c.audio,
        data: c.data,
        candidates: c.candidates,
        bundle: c.bundle,
    })
}

/// What every stack's answer must say: a sha-256 fingerprint, `active` or `passive`, ICE
/// credentials, one sendrecv audio m-line with Opus and rtcp-mux, and at least one
/// candidate (the desk never trickles: the phone applies one signed answer).
pub fn check_answer(sdp: &str) -> Result<AnswerFacts, SdpError> {
    let c = parse_common(sdp, false)?;
    if c.setup == Setup::ActPass {
        return Err(SdpError::AnswerSetup);
    }
    if c.audio.direction != Direction::SendRecv {
        return Err(SdpError::AnswerNotSendRecv);
    }
    if c.candidates.is_empty() {
        return Err(SdpError::NoCandidate);
    }
    Ok(AnswerFacts {
        fingerprint: c.fingerprint,
        setup: c.setup,
        ice_ufrag: c.ice_ufrag,
        ice_lite: c.ice_lite,
        audio: c.audio,
        has_data: c.data.is_some(),
        candidates: c.candidates,
    })
}

/// The sha-256 `a=fingerprint` in an SDP, session or media level, else the first one.
/// What the phone pins.
pub fn fingerprint_of(sdp: &str) -> Option<Fingerprint> {
    let all: Vec<Fingerprint> = sdp
        .lines()
        .map(|l| l.trim())
        .filter_map(|l| {
            l.strip_prefix("a=fingerprint:")
                .and_then(Fingerprint::parse)
        })
        .collect();
    all.iter().find(|f| f.is_sha256()).or(all.first()).cloned()
}

/// Strip the `candidate:` prefix a browser sends (`PATCH /offer`); `None` for the empty
/// end-of-candidates.
pub fn bare_candidate(candidate: &str) -> Option<&str> {
    let c = candidate.trim();
    let c = c.strip_prefix("a=").unwrap_or(c);
    let c = c.strip_prefix("candidate:").unwrap_or(c);
    if c.is_empty() { None } else { Some(c) }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// What aiortc and react-native-webrtc offer, shortened to the lines that matter.
    pub(crate) const AIORTC_OFFER: &str = "v=0\r\n\
o=- 3929352111 3929352111 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0 1\r\n\
a=msid-semantic:WMS *\r\n\
m=audio 52813 UDP/TLS/RTP/SAVPF 96 0 8\r\n\
c=IN IP4 127.0.0.1\r\n\
a=sendrecv\r\n\
a=extmap:1 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
a=extmap:2 urn:ietf:params:rtp-hdrext:ssrc-audio-level\r\n\
a=mid:0\r\n\
a=msid:7d1a2 a1b2c\r\n\
a=rtcp:9 IN IP4 0.0.0.0\r\n\
a=rtcp-mux\r\n\
a=ssrc:1234 cname:abc\r\n\
a=rtpmap:96 opus/48000/2\r\n\
a=fmtp:96 minptime=10;useinbandfec=1\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=candidate:9f1e 1 udp 2130706431 127.0.0.1 52813 typ host\r\n\
a=candidate:aa11 1 udp 2130706431 192.168.1.20 52813 typ host\r\n\
a=end-of-candidates\r\n\
a=ice-ufrag:kS9Z\r\n\
a=ice-pwd:1u2bFJ4cXl8Q0wzH0vY9eJ\r\n\
a=fingerprint:sha-384 05:47:8C:8F:37:9A:77:35:F7:7C:7B:75:33:6F:53:A9:54:23:9D:67:20:86:43:8B:88:FE:B6:B1:87:60:3D:58:D8:88:F0:E6:8E:84:C0:7F:BD:B5:BD:5A:6F:29:50:B4\r\n\
a=fingerprint:sha-256 2B:9C:7D:1A:0E:5F:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD\r\n\
a=fingerprint:sha-512 D9:49:0C:7A:C0:7E:EF:AE:C7:5F:B6:B2:29:7F:86:58:E9:F9:59:30:B5:17:D5:11:27:A4:00:74:9E:B2:42:79:B5:7E:4B:2E:E3:73:DE:9E:90:9B:E8:99:83:E6:64:8A:F0:72:46:04:36:5D:62:67:EC:AD:82:1D:7F:42:2D:09\r\n\
a=setup:actpass\r\n\
m=application 52813 UDP/DTLS/SCTP webrtc-datachannel\r\n\
c=IN IP4 127.0.0.1\r\n\
a=mid:1\r\n\
a=sctp-port:5000\r\n\
a=max-message-size:65536\r\n\
a=candidate:9f1e 1 udp 2130706431 127.0.0.1 52813 typ host\r\n\
a=end-of-candidates\r\n\
a=ice-ufrag:kS9Z\r\n\
a=ice-pwd:1u2bFJ4cXl8Q0wzH0vY9eJ\r\n\
a=fingerprint:sha-256 2B:9C:7D:1A:0E:5F:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD\r\n\
a=setup:actpass\r\n";

    #[test]
    fn parses_the_phones_offer() {
        let o = parse_offer(AIORTC_OFFER).unwrap();
        assert_eq!(o.ice_ufrag, "kS9Z");
        assert_eq!(o.ice_pwd, "1u2bFJ4cXl8Q0wzH0vY9eJ");
        assert!(!o.ice_lite && !o.trickle);
        assert!(o.fingerprint.is_sha256());
        assert!(o.fingerprint.value.starts_with("2B:9C:7D"));
        assert_eq!(o.setup, Setup::ActPass);
        assert_eq!(o.audio.mid, "0");
        assert_eq!(o.audio.opus_pt, 96);
        assert_eq!(o.audio.direction, Direction::SendRecv);
        assert_eq!(
            o.audio.opus_fmtp.as_deref(),
            Some("minptime=10;useinbandfec=1")
        );
        assert_eq!(o.data.as_ref().unwrap().mid, "1");
        assert_eq!(o.data.as_ref().unwrap().sctp_port, Some(5000));
        assert_eq!(o.candidates.len(), 3);
        assert_eq!(o.bundle, vec!["0", "1"]);
    }

    #[test]
    fn session_level_attributes_apply_to_the_media() {
        let sdp = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
a=ice-ufrag:u\r\na=ice-pwd:p\r\na=ice-options:trickle\r\n\
a=fingerprint:sha-256 AA:BB\r\na=setup:actpass\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:a\r\na=rtcp-mux\r\na=rtpmap:111 OPUS/48000/2\r\n";
        let o = parse_offer(sdp).unwrap();
        assert_eq!(o.audio.opus_pt, 111);
        assert!(o.trickle && o.candidates.is_empty() && o.data.is_none());
        assert_eq!(o.fingerprint.value, "AA:BB");
    }

    #[test]
    fn refuses_what_the_phone_never_sends() {
        let cases: [(&str, SdpError); 8] = [
            ("hello", SdpError::NotSdp),
            (
                "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
                SdpError::NoAudio,
            ),
            (
                "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 0\r\na=rtpmap:0 PCMU/8000\r\n",
                SdpError::NoOpus,
            ),
            (
                "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 opus/48000/2\r\n",
                SdpError::NoMid,
            ),
            (
                "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 96\r\na=mid:0\r\na=rtpmap:96 opus/48000/2\r\n",
                SdpError::NoRtcpMux,
            ),
            (
                "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 96\r\na=mid:0\r\na=rtcp-mux\r\na=rtpmap:96 opus/48000/2\r\n",
                SdpError::NoFingerprint,
            ),
            (
                "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 96\r\na=mid:0\r\na=rtcp-mux\r\na=rtpmap:96 opus/48000/2\r\na=fingerprint:sha-1 AA\r\n",
                SdpError::NotSha256,
            ),
            (
                "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 96\r\na=mid:0\r\na=rtcp-mux\r\na=rtpmap:96 opus/48000/2\r\na=fingerprint:sha-256 AA\r\n",
                SdpError::NoIce,
            ),
        ];
        for (sdp, err) in cases {
            assert_eq!(parse_offer(sdp).unwrap_err(), err, "{sdp:?}");
        }
        let two_audio = AIORTC_OFFER.replace("m=application", "m=audio");
        assert_eq!(parse_offer(&two_audio).unwrap_err(), SdpError::TooManyAudio);
        let passive = AIORTC_OFFER.replace("a=setup:actpass", "a=setup:passive");
        assert_eq!(parse_offer(&passive).unwrap_err(), SdpError::OfferSetup);
        let no_cand = AIORTC_OFFER
            .lines()
            .filter(|l| !l.starts_with("a=candidate"))
            .collect::<Vec<_>>()
            .join("\r\n");
        assert_eq!(
            parse_offer(&no_cand).unwrap_err(),
            SdpError::NoCandidateNoTrickle
        );
    }

    #[test]
    fn fingerprint_parses_normalises_and_prints() {
        let f = Fingerprint::parse("SHA-256 ab:cd:EF").unwrap();
        assert_eq!(f.algorithm, "sha-256");
        assert_eq!(f.value, "AB:CD:EF");
        assert_eq!(f.to_string(), "sha-256 AB:CD:EF");
        assert_eq!(
            Fingerprint::parse("a=fingerprint:sha-256 AB:CD"),
            Some(Fingerprint {
                algorithm: "sha-256".into(),
                value: "AB:CD".into()
            })
        );
        assert!(Fingerprint::parse("sha-256").is_none());
        assert!(Fingerprint::parse("sha-256 ABC").is_none());
        assert!(Fingerprint::parse("sha-256 AB:ZZ").is_none());
        assert!(Fingerprint::parse("sha-256 AB:CD extra").is_none());
        // sha-256 of the empty string, as SDP writes it
        let empty = Fingerprint::from_der(b"");
        assert!(empty.value.starts_with("E3:B0:C4:42:98:FC:1C:14"));
        assert_eq!(empty.value.len(), 32 * 3 - 1);
        assert_eq!(fingerprint_of(AIORTC_OFFER).unwrap().value.len(), 95);
        assert!(fingerprint_of("v=0\r\n").is_none());
    }

    #[test]
    fn checks_an_answers_shape() {
        let answer = AIORTC_OFFER.replace("a=setup:actpass", "a=setup:active");
        let facts = check_answer(&answer).unwrap();
        assert_eq!(facts.setup, Setup::Active);
        assert!(facts.has_data && facts.candidates.len() == 3);
        assert_eq!(facts.audio.opus_pt, 96);
        assert_eq!(
            check_answer(AIORTC_OFFER).unwrap_err(),
            SdpError::AnswerSetup
        );
        let recvonly = answer.replace("a=sendrecv", "a=recvonly");
        assert_eq!(
            check_answer(&recvonly).unwrap_err(),
            SdpError::AnswerNotSendRecv
        );
        let no_cand = answer
            .lines()
            .filter(|l| !l.starts_with("a=candidate"))
            .collect::<Vec<_>>()
            .join("\r\n");
        assert_eq!(check_answer(&no_cand).unwrap_err(), SdpError::NoCandidate);
    }

    #[test]
    fn bare_candidates() {
        assert_eq!(
            bare_candidate("candidate:1 1 udp 1 127.0.0.1 9 typ host"),
            Some("1 1 udp 1 127.0.0.1 9 typ host")
        );
        assert_eq!(bare_candidate("a=candidate:x"), Some("x"));
        assert_eq!(bare_candidate("x"), Some("x"));
        assert_eq!(bare_candidate(""), None);
        assert_eq!(bare_candidate("candidate:"), None);
    }
}
