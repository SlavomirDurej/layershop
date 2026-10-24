use super::*;
use crate::fetch::{Fetch, Head, Request as FetchRequest, Sink};
use photocraft_aigen::{Method, Request, Response, Transport};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Hugging Face's OAuth endpoints, as a fake: answers by address, and keeps every request (its
/// address, whether it carried a token, and its body).
#[derive(Default)]
struct FakeHub {
    seen: Mutex<Vec<(String, bool, String)>>,
    /// Answers to the token endpoint, in turn (the last one repeats).
    token_answers: Mutex<Vec<(u16, String)>>,
}

impl FakeHub {
    fn with_tokens(answers: &[(u16, &str)]) -> Self {
        Self { token_answers: Mutex::new(answers.iter().map(|(s, b)| (*s, b.to_string())).collect()), ..Self::default() }
    }

    fn seen(&self) -> Vec<(String, bool, String)> {
        self.seen.lock().unwrap().clone()
    }
}

impl Transport for FakeHub {
    fn send(&self, r: &Request) -> photocraft_aigen::Result<Response> {
        let auth = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.clone());
        self.seen.lock().unwrap().push((r.url.clone(), auth.is_some(), String::from_utf8_lossy(&r.body).into_owned()));
        let answer = |status: u16, body: &str| Ok(Response { status, body: body.as_bytes().to_vec() });
        if r.url == TOKEN_URL && r.method == Method::Post {
            let mut answers = self.token_answers.lock().unwrap();
            let (status, body) = if answers.len() > 1 { answers.remove(0) } else { answers.first().cloned().unwrap_or((500, String::new())) };
            return answer(status, &body);
        }
        match (r.url.as_str(), auth.as_deref()) {
            (USERINFO_URL, Some("Bearer hf_oauth_fresh" | "Bearer hf_oauth_first")) => answer(200, r#"{"sub":"123","preferred_username":"pixel-painter","name":"P"}"#),
            (WHOAMI_URL, Some("Bearer hf_pasted_good")) => answer(200, r#"{"name":"token-owner","type":"user"}"#),
            (USERINFO_URL | WHOAMI_URL, _) => answer(401, r#"{"error":"Invalid username or password."}"#),
            _ => answer(404, ""),
        }
    }
}

const FIRST: &str = r#"{"access_token":"hf_oauth_first","token_type":"bearer","expires_in":28800,"refresh_token":"hf_refresh_1","scope":"openid profile gated-repos","id_token":"x.y.z"}"#;
const FRESH: &str = r#"{"access_token":"hf_oauth_fresh","token_type":"bearer","expires_in":28800,"refresh_token":"hf_refresh_2"}"#;

#[test]
fn pkce_and_the_sign_in_address_are_as_the_specs_write_them() {
    // base64url(sha256(verifier)), as Python's hashlib and base64 compute it.
    assert_eq!(challenge("dBjftJeZ4CVP-mJ92K1-I3eMFe1bjOJYqUvVlqDuVg0"), "e4TEPjrm8mJTSiI1kANHtKI1a8jK38rblVzQJvMuZM0");
    assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    assert_eq!(base64url(b"any carnal pleas"), "YW55IGNhcm5hbCBwbGVhcw");
    assert_eq!(base64url(&[]), "");
    let url = authorize_url("my-app-id", "http://127.0.0.1:53111/callback", "st4te", "ch4llenge");
    assert!(url.starts_with("https://huggingface.co/oauth/authorize?client_id=my-app-id&"), "{url}");
    for part in [
        "redirect_uri=http%3A%2F%2F127.0.0.1%3A53111%2Fcallback",
        "response_type=code",
        "scope=openid%20profile%20gated-repos",
        "state=st4te",
        "code_challenge=ch4llenge",
        "code_challenge_method=S256",
    ] {
        assert!(url.contains(part), "{part} in {url}");
    }
    let pairs = parse_query("code=a%2Bb+c&state=x%3D1&odd=%zz&empty&&");
    assert_eq!(
        pairs,
        [("code".into(), "a+b c".into()), ("state".into(), "x=1".into()), ("odd".into(), "%zz".into()), ("empty".into(), String::new())]
    );
    assert_eq!(decode("%E2%80%A6%FF"), "\u{2026}\u{fffd}");
    assert_eq!(decode(&encode("a b/ü&=")), "a b/ü&=");
}

#[test]
fn the_code_becomes_tokens_and_a_name_and_they_are_renewed_before_they_expire() {
    let hub = FakeHub::with_tokens(&[(200, FIRST), (200, FRESH)]);
    let now = 1_800_000_000;
    let mut c = exchange(&hub, "my-app-id", "the-code", "the-verifier", "http://127.0.0.1:5/callback", now).unwrap();
    assert_eq!((c.access.as_str(), c.refresh.as_str(), c.expires_at, c.via), ("hf_oauth_first", "hf_refresh_1", now + 28_800, Via::OAuth));
    let body = &hub.seen()[0].2;
    for part in ["grant_type=authorization_code", "code=the-code", "code_verifier=the-verifier", "client_id=my-app-id", "redirect_uri=http%3A%2F%2F127.0.0.1%3A5%2Fcallback"] {
        assert!(body.contains(part), "{part} in {body}");
    }
    assert!(!body.contains("client_secret"), "a public app has none");
    c.user = userinfo(&hub, &c.access).unwrap();
    assert_eq!(c.user, "pixel-painter");
    // Hours later, a few minutes before it expires: renewed, the name kept.
    assert!(!c.needs_refresh(now + 3600));
    assert!(c.needs_refresh(c.expires_at - 60));
    let fresh = refresh(&hub, "my-app-id", &c, c.expires_at - 60).unwrap();
    assert_eq!((fresh.access.as_str(), fresh.refresh.as_str(), fresh.user.as_str()), ("hf_oauth_fresh", "hf_refresh_2", "pixel-painter"));
    assert!(hub.seen().last().unwrap().2.contains("grant_type=refresh_token&refresh_token=hf_refresh_1"));
    // A refresh Hugging Face refuses means signing in again; so does one with nothing to renew.
    let refused = FakeHub::with_tokens(&[(400, r#"{"error":"invalid_grant"}"#)]);
    assert!(matches!(refresh(&refused, "my-app-id", &fresh, now), Err(Error::Access(m)) if m.contains("Sign in again")));
    assert!(matches!(refresh(&hub, "id", &Credentials { refresh: String::new(), ..fresh.clone() }, now), Err(Error::Access(_))));
    // A token without an expiry, or pasted by hand, is never "about to expire".
    assert!(!Credentials { expires_at: 0, ..fresh.clone() }.needs_refresh(u64::MAX));
    assert!(!Credentials { via: Via::Token, ..fresh.clone() }.needs_refresh(u64::MAX));
    // Run out with a refresh token: renewed, not expired. Without one (Hugging Face sends none, or
    // a token pasted by hand that was given an end): expired.
    assert!(!fresh.expired(u64::MAX));
    assert!(Credentials { refresh: String::new(), ..fresh.clone() }.expired(fresh.expires_at));
    assert!(!Credentials { refresh: String::new(), ..fresh.clone() }.expired(fresh.expires_at - 1));
    assert!(!Credentials { refresh: String::new(), expires_at: 0, ..fresh.clone() }.expired(u64::MAX), "no end known: not expired");
    assert!(!Credentials::default().expired(u64::MAX), "signed out is not expired");
    // A refresh without a new refresh token keeps the old one.
    assert_eq!(parse_tokens(br#"{"access_token":"hf_a","expires_in":10}"#, 5, "keep").unwrap().refresh, "keep");
    for junk in [&b""[..], b"[]", b"{}", br#"{"access_token":""}"#, br#"{"access_token":"has space"}"#, b"<html>"] {
        assert!(parse_tokens(junk, 0, "").is_err(), "{}", String::from_utf8_lossy(junk));
    }
    // An exchange Hugging Face refuses says why.
    let bad = FakeHub::with_tokens(&[(400, r#"{"error":"invalid_grant","error_description":"bad code verifier"}"#)]);
    let e = exchange(&bad, "id", "c", "v", "http://127.0.0.1:5/callback", now).unwrap_err().to_string();
    assert!(e.contains("invalid_grant") && e.contains("bad code verifier"), "{e}");
    // `Debug` never shows a token.
    assert!(!format!("{fresh:?}").contains("hf_oauth_fresh") && !format!("{fresh:?}").contains("hf_refresh_2"));
}

#[test]
fn a_pasted_token_is_checked_with_whoami() {
    let hub = FakeHub::default();
    assert_eq!(whoami(&hub, "hf_pasted_good").unwrap(), "token-owner");
    assert!(matches!(whoami(&hub, "hf_pasted_bad"), Err(Error::Access(m)) if m.contains("did not accept that access token")));
    assert!(plausible("hf_abcd") && !plausible("  ") && !plausible("two words") && !plausible("a\u{7}bc"));
}

#[test]
fn a_token_goes_to_hugging_face_and_nowhere_else() {
    let hub = FakeHub::with_tokens(&[(200, FIRST)]);
    for url in ["https://huggingface.co.evil.example/oauth/token", "http://huggingface.co/oauth/token", "https://evil.example/?https://huggingface.co/", "https://cdn-lfs.hf.co/x"] {
        let r = Request::get(url).header("Authorization", "Bearer hf_secret");
        assert!(matches!(send(&hub, &r), Err(Error::Config(_))), "{url}");
    }
    assert!(hub.seen().is_empty(), "nothing was sent");
    // The whole sign-in, from the code to the name: every request went to huggingface.co.
    let mut c = exchange(&hub, "id", "c", "v", "http://127.0.0.1:5/callback", 0).unwrap();
    c.user = userinfo(&hub, &c.access).unwrap();
    let _ = whoami(&hub, "hf_pasted_good");
    let seen = hub.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|(url, _, _)| url.starts_with("https://huggingface.co/")), "{seen:?}");
    // The downloads' own guard: the token stays on huggingface.co, and not on the CDN it redirects to.
    let policy = crate::fetch::Policy::default();
    assert_eq!(policy.token_for("https://huggingface.co/bfl/x/resolve/main/f", Some("hf_t")), Some("hf_t"));
    for host in ["https://cdn-lfs.hf.co/f", "https://us.aws.cdn.hf.co/x", "https://github.com/x", "https://evil.example/f"] {
        assert_eq!(policy.token_for(host, Some("hf_t")), None, "{host}");
    }
}

/// Answers each `HEAD` of a gated file with a status (and Hugging Face's error code), and keeps
/// the tokens it was sent.
struct FakeFiles {
    status: u16,
    code: Option<&'static str>,
    tokens: Mutex<Vec<Option<String>>>,
}

impl FakeFiles {
    fn new(status: u16, code: Option<&'static str>) -> Self {
        Self { status, code, tokens: Mutex::new(Vec::new()) }
    }
}

impl Fetch for FakeFiles {
    fn head(&self, _: &str, token: Option<&str>) -> crate::Result<Head> {
        self.tokens.lock().unwrap().push(token.map(str::to_string));
        Ok(Head { status: self.status, error_code: self.code.map(str::to_string), error_message: Some("its own words".into()), ..Head::default() })
    }
    fn fetch(&self, _: &FetchRequest<'_>, _: &mut dyn Sink, _: &dyn Fn() -> bool) -> crate::Result<()> {
        panic!("checking access downloads nothing")
    }
}

#[test]
fn each_gated_model_is_sign_in_then_licence_then_ready() {
    let m = Manifest::bundled().unwrap();
    let recommended = m.presets["recommended"].clone();
    assert_eq!(gated(m, &recommended).iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["FLUX.3 Klein"]);
    let state = |status: u16, code: Option<&'static str>, token: Option<&str>| {
        let files = FakeFiles::new(status, code);
        let found = access(m, &recommended, token, &files).unwrap();
        assert_eq!(found.len(), 1, "only the gated one is asked about");
        (found[0].access, found[0].detail.clone(), files.tokens.lock().unwrap().clone())
    };
    // No sign-in: nothing is asked.
    let (a, _, asked) = state(200, None, None);
    assert_eq!((a, asked.len()), (Access::SignIn, 0));
    // As Hugging Face answers: 401 GatedRepo signed out (or a token it doesn't take), 403 GatedRepo
    // signed in without the licence, 2xx or the CDN redirect ready.
    assert_eq!(state(401, Some("GatedRepo"), Some("hf_x")).0, Access::SignIn);
    let (a, detail, asked) = state(403, Some("GatedRepo"), Some("hf_x"));
    assert_eq!((a, detail.as_str(), asked), (Access::Licence, "", vec![Some("hf_x".to_string())]));
    assert_eq!(state(200, None, Some("hf_x")).0, Access::Ready);
    assert_eq!(state(302, None, Some("hf_x")).0, Access::Ready);
    // Signed in, a file that is not there any more: that model can't come, and it says why.
    let (a, detail, _) = state(404, Some("EntryNotFound"), Some("hf_x"));
    assert_eq!(a, Access::Unavailable);
    assert!(detail.contains("FLUX.3 Klein") && detail.contains("newer version"), "{detail}");
    assert_eq!(state(410, None, Some("hf_x")).0, Access::Unavailable);
    // A 403 that is not about the licence says what it is.
    assert_eq!(state(403, Some("TokenPermission"), Some("hf_x")).1, "its own words");
    // Anything else is an error, not a state.
    assert!(access(m, &recommended, Some("hf_x"), &FakeFiles::new(500, None)).is_err());
    // The ungated ones are not asked about at all.
    assert!(access(m, &["qwen-image-2-1".to_string(), "no-such".to_string()], Some("hf_x"), &FakeFiles::new(200, None)).unwrap().is_empty());
    let klein = &access(m, &recommended, None, &FakeFiles::new(200, None)).unwrap()[0];
    assert!(klein.page.starts_with("https://huggingface.co/") && klein.licence.contains("Kommunity"), "{klein:?}");
}

/// The browser coming back to the sign-in's listener with `target`; what the page said.
fn browser(url: &str, target: &str) -> String {
    let port = url.split("127.0.0.1%3A").nth(1).and_then(|r| r.split("%2F").next()).unwrap();
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
    s.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).unwrap();
    let mut page = String::new();
    let _ = s.read_to_string(&mut page);
    page
}

fn state_of(url: &str) -> String {
    let query = url.split_once('?').unwrap().1;
    parse_query(query).into_iter().find(|(k, _)| k == "state").unwrap().1
}

fn ended(sign_in: &SignIn) -> crate::Result<Credentials> {
    let began = Instant::now();
    loop {
        if let Some(r) = sign_in.take() {
            return r;
        }
        assert!(began.elapsed() < Duration::from_secs(10), "the sign-in did not end");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn signing_in_through_the_browser_ends_with_a_name() {
    let hub = Arc::new(FakeHub::with_tokens(&[(200, FIRST)]));
    let sign_in = SignIn::start(hub.clone(), "my-app-id", "LayerShop", Duration::from_secs(20)).unwrap();
    let url = sign_in.url().to_string();
    assert!(url.starts_with(AUTHORIZE_URL) && url.contains("code_challenge_method=S256"), "{url}");
    // A favicon first: answered, and the wait goes on.
    assert!(browser(&url, "/favicon.ico").starts_with("HTTP/1.1 404"));
    let page = browser(&url, &format!("/callback?code=abc123&state={}", encode(&state_of(&url))));
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("Signed in to Hugging Face") && page.contains("go back to LayerShop"), "{page}");
    let c = ended(&sign_in).unwrap();
    assert_eq!((c.access.as_str(), c.user.as_str(), c.via), ("hf_oauth_first", "pixel-painter", Via::OAuth));
    // The code went to the token endpoint with the verifier whose challenge was sent.
    let body = hub.seen()[0].2.clone();
    let verifier = parse_query(&body).into_iter().find(|(k, _)| k == "code_verifier").unwrap().1;
    assert!(url.contains(&format!("code_challenge={}", challenge(&verifier))), "{url}");
    assert!(body.contains("code=abc123"));
}

#[test]
fn a_state_that_is_not_ours_is_refused() {
    let hub = Arc::new(FakeHub::with_tokens(&[(200, FIRST)]));
    let sign_in = SignIn::start(hub.clone(), "my-app-id", "LayerShop", Duration::from_secs(20)).unwrap();
    let page = browser(sign_in.url(), "/callback?code=abc123&state=someone-elses");
    assert!(page.starts_with("HTTP/1.1 400") && page.contains("refused"), "{page}");
    assert!(matches!(ended(&sign_in), Err(Error::Access(m)) if m.contains("did not match")));
    assert!(hub.seen().is_empty(), "the code was never exchanged");
    // The user saying no in the browser.
    let sign_in = SignIn::start(hub.clone(), "my-app-id", "LayerShop", Duration::from_secs(20)).unwrap();
    let url = sign_in.url().to_string();
    browser(&url, &format!("/callback?error=access_denied&state={}", encode(&state_of(&url))));
    assert!(matches!(ended(&sign_in), Err(Error::Access(m)) if m.contains("cancelled")));
}

#[test]
fn a_sign_in_times_out_or_is_cancelled() {
    let hub: Arc<dyn Transport> = Arc::new(FakeHub::default());
    let sign_in = SignIn::start(hub.clone(), "my-app-id", "LayerShop", Duration::from_millis(150)).unwrap();
    assert!(matches!(ended(&sign_in), Err(Error::Access(m)) if m.contains("timed out")));
    let sign_in = SignIn::start(hub.clone(), "my-app-id", "LayerShop", Duration::from_secs(60)).unwrap();
    sign_in.cancel();
    assert_eq!(ended(&sign_in).unwrap_err(), Error::Cancelled);
    // Without a registered app there is no sign-in to start.
    assert!(matches!(SignIn::start(hub, "  ", "LayerShop", Duration::from_secs(1)), Err(Error::Config(m)) if m.contains("access token")));
}

#[test]
fn the_listener_takes_odd_requests_without_falling_over() {
    let hub = Arc::new(FakeHub::with_tokens(&[(200, FIRST)]));
    let sign_in = SignIn::start(hub, "my-app-id", "LayerShop", Duration::from_secs(20)).unwrap();
    let url = sign_in.url().to_string();
    let port: u16 = url.split("127.0.0.1%3A").nth(1).and_then(|r| r.split("%2F").next()).unwrap().parse().unwrap();
    // Garbage, a POST, a connection that says nothing: each answered or dropped, and the wait goes on.
    for junk in [&b"\x00\xff\xfe garbage\r\n\r\n"[..], b"POST /callback HTTP/1.1\r\n\r\n", b""] {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(junk).unwrap();
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut sink = Vec::new();
        let _ = s.read_to_end(&mut sink);
    }
    assert!(sign_in.take().is_none(), "still waiting");
    browser(&url, &format!("/callback?code=ok&state={}", encode(&state_of(&url))));
    assert!(ended(&sign_in).is_ok());
}
