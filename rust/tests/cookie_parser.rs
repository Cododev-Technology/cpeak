// Parsing and setting cookies with cookie_parser
mod common;

use common::{request, serve};
use cpeak::{cookie_parser, cpeak, sign_cookie, CookieOptions, CookieParserOptions, SameSite};
use serde_json::json;

const SECRET: &str = "test-secret-cpeak-cookie-signing!!";

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.before_each(cookie_parser(None));

        server.route("get", "/get-cookies", async |req, res| {
            res.status(200)
                .json(json!({ "receivedCookies": req.cookies }))
        });

        server
    })
}

fn get_cookies(cookie: &str) -> serde_json::Value {
    request(server(), "GET", "/get-cookies")
        .header("Cookie", cookie)
        .send()
        .json()["receivedCookies"]
        .clone()
}

#[test]
fn should_handle_basic_cookies() {
    assert_eq!(
        get_cookies("foo=bar; baz=qux"),
        json!({ "foo": "bar", "baz": "qux" })
    );
}

#[test]
fn should_handle_quoted_values_stripping_double_quotes() {
    assert_eq!(
        get_cookies("id=\"12345\"; user=jane"),
        json!({ "id": "12345", "user": "jane" })
    );
}

#[test]
fn should_handle_encoded_values_utf8_url_decoding() {
    assert_eq!(
        get_cookies("city=New%20York; fav=%E2%9C%93"),
        json!({ "city": "New York", "fav": "✓" })
    );
}

#[test]
fn should_handle_malformed_percent_encoding_gracefully() {
    // Should return raw string if decoding fails to avoid crashing
    assert_eq!(
        get_cookies("bad=%zz; good=yes"),
        json!({ "bad": "%zz", "good": "yes" })
    );
}

#[test]
fn should_handle_extreme_whitespace_and_empty_pairs() {
    assert_eq!(
        get_cookies("  space = cadet ; "),
        json!({ "space": "cadet" })
    );
}

#[test]
fn should_handle_valueless_cookies() {
    assert_eq!(
        get_cookies("flag=; session=1"),
        json!({ "flag": "", "session": "1" })
    );
}

#[test]
fn should_handle_duplicates_by_honoring_the_first_occurrence() {
    // RFC 6265: Servers SHOULD use the first appearance
    assert_eq!(
        get_cookies("pref=light; pref=dark"),
        json!({ "pref": "light" })
    );
}

#[test]
fn should_treat_a_lone_double_quote_as_a_literal_value_not_strip_it_to_empty_string() {
    assert_eq!(get_cookies("x=\""), json!({ "x": "\"" }));
}

#[test]
fn should_not_silently_drop_a_cookie_whose_name_is_proto() {
    let cookies = get_cookies("__proto__=secret; legit=yes");
    assert_eq!(cookies["legit"], "yes");
    assert_eq!(cookies["__proto__"], "secret");
}

#[test]
fn should_not_drop_cookies_whose_names_match_object_prototype_methods() {
    let cookies = get_cookies("toString=a; hasOwnProperty=b; valueOf=c; constructor=d; legit=ok");
    assert_eq!(cookies["legit"], "ok");
    assert_eq!(cookies["toString"], "a");
    assert_eq!(cookies["hasOwnProperty"], "b");
    assert_eq!(cookies["valueOf"], "c");
    assert_eq!(cookies["constructor"], "d");
}

#[test]
fn should_treat_cookie_names_as_case_sensitive() {
    assert_eq!(
        get_cookies("Foo=upper; foo=lower"),
        json!({ "Foo": "upper", "foo": "lower" })
    );
}

#[test]
fn should_preserve_equal_signs_inside_cookie_values_base64_jwt_padding() {
    assert_eq!(
        get_cookies("token=YWJjPT0=; jwt=eyJhbGciOiJIUzI1NiJ9.payload.sig="),
        json!({ "token": "YWJjPT0=", "jwt": "eyJhbGciOiJIUzI1NiJ9.payload.sig=" })
    );
}

#[test]
fn should_ignore_pairs_that_lack_an_equal_sign() {
    assert_eq!(
        get_cookies("noequalshere; real=ok; alsoBare"),
        json!({ "real": "ok" })
    );
}

#[test]
fn should_preserve_plus_as_a_literal_character_not_decode_it_to_space() {
    assert_eq!(
        get_cookies("q=a+b+c; mixed=hello+world%20foo"),
        json!({ "q": "a+b+c", "mixed": "hello+world foo" })
    );
}

#[test]
fn should_tolerate_multiple_consecutive_semicolon_separators() {
    assert_eq!(get_cookies(";;a=1;;;b=2;;"), json!({ "a": "1", "b": "2" }));
}

// Setting cookies with cookie_parser

fn server2() -> u16 {
    serve(|| {
        let mut server2 = cpeak();
        server2.before_each(cookie_parser(CookieParserOptions {
            secret: Some(SECRET.into()),
        }));

        server2.route("get", "/set-cookie", async |_req, res| {
            res.cookie(
                "session",
                "abc123",
                CookieOptions {
                    domain: Some("example.com".into()),
                    max_age: Some(86400000),
                    expires: Some(
                        httpdate::parse_http_date("Thu, 31 Dec 2026 00:00:00 GMT").unwrap(),
                    ),
                    http_only: true,
                    secure: true,
                    same_site: Some(SameSite::Strict),
                    path: Some("/dashboard".into()),
                    ..Default::default()
                },
            )
            .cookie(
                "token",
                "secret-val",
                CookieOptions {
                    signed: true,
                    http_only: true,
                    path: Some("/".into()),
                    ..Default::default()
                },
            )
            .status(200)
            .json(json!({ "ok": true }))
        });

        server2.route("get", "/read-signed", async |req, res| {
            res.json(json!({ "token": req.signed_cookies.get("token"), "cookies": req.cookies }))
        });

        server2.route("get", "/clear-cookie", async |_req, res| {
            res.clear_cookie("token", None).json(json!({ "ok": true }))
        });

        server2
    })
}

#[test]
fn should_set_a_plain_cookie_with_all_options_and_a_signed_cookie() {
    let res = request(server2(), "GET", "/set-cookie").send();
    let headers = res.headers_all("set-cookie");

    // Plain cookie: all attributes present
    assert!(headers[0].starts_with("session=abc123"));
    assert!(headers[0].contains("Path=/dashboard"));
    assert!(headers[0].contains("Domain=example.com"));
    assert!(headers[0].contains("Max-Age=86400"));
    assert!(headers[0].contains("Expires=Thu, 31 Dec 2026 00:00:00 GMT"));
    assert!(headers[0].contains("HttpOnly"));
    assert!(headers[0].contains("Secure"));
    assert!(headers[0].contains("SameSite=Strict"));

    // Signed cookie: value is s:<original>.<hmac> URL-encoded, attributes intact
    assert!(headers[1].starts_with("token=s%3Asecret-val."));
    assert!(headers[1].contains("HttpOnly"));
    assert!(headers[1].contains("Path=/"));
}

#[test]
fn should_read_back_a_signed_cookie_and_flag_a_tampered_one() {
    let res = request(server2(), "GET", "/set-cookie").send();
    let signed = res.headers_all("set-cookie")[1]
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let valid = request(server2(), "GET", "/read-signed")
        .header("Cookie", &format!("{signed}; theme=dark"))
        .send();
    assert_eq!(
        valid.json(),
        json!({ "token": "secret-val", "cookies": { "theme": "dark" } })
    );

    let tampered = request(server2(), "GET", "/read-signed")
        .header("Cookie", "token=s%3Aanother-val.AAAA")
        .send();
    assert_eq!(tampered.json(), json!({ "token": null, "cookies": {} }));
}

#[test]
fn should_sign_a_value_the_way_res_cookie_does() {
    // crypto.createHmac("sha256", secret).update(value).digest("base64url") in Node.js
    assert_eq!(
        sign_cookie("abc123", SECRET),
        "s:abc123.dh3HD4sz_xmtvFcVaDW6pitEipV4gE7_jcQrdcfX7jY"
    );

    // What res.cookie() sets is the same value, URL-encoded
    let res = request(server2(), "GET", "/set-cookie").send();
    let set = res.headers_all("set-cookie")[1].split(';').next().unwrap();
    assert_eq!(
        set,
        format!("token={}", sign_cookie("secret-val", SECRET)).replace(':', "%3A")
    );
}

#[test]
fn should_read_a_cookie_that_was_signed_with_sign_cookie() {
    let res = request(server2(), "GET", "/read-signed")
        .header(
            "Cookie",
            &format!("token={}", sign_cookie("user-42", SECRET)),
        )
        .send();
    assert_eq!(res.json(), json!({ "token": "user-42", "cookies": {} }));

    // Signed with another secret, it's not one of ours
    let other = request(server2(), "GET", "/read-signed")
        .header(
            "Cookie",
            &format!(
                "token={}",
                sign_cookie("user-42", "another-secret-of-32-characters!!")
            ),
        )
        .send();
    assert_eq!(other.json(), json!({ "token": null, "cookies": {} }));
}

#[test]
fn should_clear_a_cookie() {
    let res = request(server2(), "GET", "/clear-cookie").send();
    assert_eq!(
        res.header("set-cookie"),
        Some("token=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT")
    );
}

#[test]
#[should_panic(expected = "CPEAK_ERR_WEAK_SECRET")]
fn should_throw_weak_secret_for_a_short_secret() {
    let _ = cookie_parser(CookieParserOptions {
        secret: Some("your-secret-key".into()),
    });
}
