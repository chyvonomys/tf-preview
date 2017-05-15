extern crate iron;
extern crate persistent;
extern crate router;
extern crate hyper;
extern crate hyper_native_tls;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate select;
extern crate params;

use iron::prelude::*;
use iron::status;
use iron::typemap::Key;

use std::collections::HashMap;

use hyper::header::ContentType;
use hyper::net::HttpsConnector;
use hyper_native_tls::NativeTlsClient as TlsClient;

#[derive(Serialize, Debug, Clone)]
struct OgPreviewRes {
    ok: bool,
    #[serde(skip_serializing_if="Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    image: Option<String>,
    images: Vec<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    cached: Option<bool>,
}

impl OgPreviewRes {
    fn new() -> OgPreviewRes {
        OgPreviewRes{
            ok: false,
            title: None,
            image: None,
            images: Vec::new(),
            description: None,
            cached: None,
        }
    }
}

struct LinkCache {
    cache: HashMap<String, OgPreviewRes>,
}

impl Key for LinkCache {
    type Value = LinkCache;
}

fn parse_body(resp: hyper::client::Response) -> OgPreviewRes {
    let mut res = OgPreviewRes::new();
    
    use select::document::Document;
    use select::predicate::{Attr, Name};

    match Document::from_read(resp) {
        Ok(document) => {

            res.ok = true;
            res.cached = Some(false);

            if let Some(og_ti) = document.find(Attr("property", "og:title")).take(1).next() {
                if let Some(ti) = og_ti.attr("content") {
                    res.title = Some(String::from(ti));
                }
            } else if let Some(ti) = document.find(Name("title")).take(1).next() {
                res.title = Some(ti.inner_html());
            }

            if let Some(og_de) = document.find(Attr("property", "og:description")).take(1).next() {
                if let Some(de) = og_de.attr("content") {
                    res.description = Some(String::from(de));
                }
            } else if let Some(_de) = document.find(Attr("name", "description")).take(1).next() {
                if let Some(de) = _de.attr("content") {
                    res.description = Some(String::from(de));
                }
            }

            if let Some(og_im) = document.find(Attr("property", "og:image")).take(1).next() {
                if let Some(im) = og_im.attr("content") {
                    res.image = Some(String::from(im));
                }
            }

            res.images = document.find(Attr("property", "og:image"))
                .filter_map(|oimg| oimg.attr("content"))
                .map(|s| String::from(s))
                .collect();
        },
        Err(r) => println!("parsing document error: {}", r),
    }
    res
}

fn build_preview(url: &str) -> OgPreviewRes {
    println!("url: {}", url);

    let ssl = TlsClient::new().unwrap();
    let connector = HttpsConnector::new(ssl);
    let client = hyper::Client::with_connector(connector);

    match client.head(url).send() {
        Ok(head_resp) => {
            use hyper::header::ContentType;
            use hyper::mime::{Mime, TopLevel, SubLevel};
            match head_resp.headers.get::<ContentType>() {
                Some(&ContentType(Mime(TopLevel::Text, SubLevel::Html, _))) => {
                    match client.get(url).send() {
                        Ok(get_resp) => parse_body(get_resp),
                        _ => OgPreviewRes::new(),
                    }
                },
                _ => OgPreviewRes::new(),
            }
        },
        _ => OgPreviewRes::new(),
    }
}

#[test]
fn test_build_preview() {
    let resp1 = build_preview("http://github.com");
    println!("resp 1: {:#?}", resp1);
    assert_eq!(match resp1 {
        OgPreviewRes{
            ok: true,
            title: Some(_),
            image: Some(_),
            ref images,
            description: Some(_),
            cached: Some(false),
        } if images.len() > 0 => true,
        _ => false,
    }, true);

    let resp2 = build_preview("bruh");
    println!("resp 2: {:#?}", resp2);
    assert_eq!(match resp2 {
        OgPreviewRes{
            ok: false,
            title: None,
            image: None,
            ref images,
            description: None,
            cached: None,
        } if images.len() == 0 => true,
        _ => false,
    }, true);
}

fn preview(req: &mut Request) -> IronResult<Response> {
    let arc_st_r = req.get::<persistent::State<LinkCache>>();
    let ps = req.get_ref::<params::Params>().unwrap();

    let mut json = String::new();

    if let Some(&params::Value::String(ref url)) = ps.find(&["url"]) {
        if let Ok(arc_st) = arc_st_r {
            let mut store: Option<OgPreviewRes> = None;

            if let Ok(guard) = arc_st.read() {
                let state = &*guard;
                if let Some(v) = state.cache.get(url) {
                    json = serde_json::to_string_pretty(v).unwrap();
                } else {
                    let mut new_res = build_preview(url);
                    json = serde_json::to_string_pretty(&new_res).unwrap();
                    new_res.cached = Some(true);
                    store = Some(new_res);
                }
            }

            if let Some(new_res) = store {
                if let Ok(mut wguard) = arc_st.write() {
                    let wstate = &mut *wguard;
                    wstate.cache.insert(url.clone(), new_res);
                }
            }
        }
    }

    if json.is_empty() {
        json = serde_json::to_string_pretty(&OgPreviewRes::new()).unwrap();
    }

    let mut resp = Response::with((status::Ok, json));
    resp.headers.set(ContentType::json());
    Ok(resp)
}

fn main() {
    let listenaddr = std::env::var("PREVIEW_LISTENADDR")
        .unwrap_or("localhost:2345".to_owned());

    let listenpath = std::env::var("PREVIEW_LISTENPATH")
        .unwrap_or("/preview".to_owned());
    
    let link_cache = LinkCache{cache: HashMap::new()};
    
    let mut router = router::Router::new();
    router.get(listenpath, preview, "preview");

    let mut chain = Chain::new(router);
    chain.link(persistent::State::<LinkCache>::both(link_cache));

    match Iron::new(chain).http(listenaddr.as_str()) {
        Ok(_) => {}
        Err(e) => println!("iron http failure {}", e.to_string())
    }
}
