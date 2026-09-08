//! Runs the Silicon IAM stub for local development and prints the seeded logins — the carbons, and
//! the silicon with its `stk-` token, because this is a dev tool and the seed is public.
//! `IAM_STUB_ADDR` (default `127.0.0.1:8099`) and `IAM_STUB_SEED` (default the embedded fixture).

use std::{io, net::SocketAddr};

use space_station_backend::iam_stub::{default_seed, seed_from_file, serve};

#[tokio::main]
async fn main() -> io::Result<()> {
    let addr: SocketAddr =
        std::env::var("IAM_STUB_ADDR").unwrap_or_else(|_| "127.0.0.1:8099".into()).parse().map_err(io::Error::other)?;
    let seed = match std::env::var_os("IAM_STUB_SEED") {
        Some(path) => seed_from_file(path)?,
        None => default_seed(),
    };
    let (addr, server) = serve(seed.clone(), addr).await?;
    let app = &seed.app.app_id;
    println!("Silicon IAM stub  http://{addr}");
    println!("application       {app}  (HTTP Basic user; the ask_ secret is in the seed)");
    for o in &seed.orgs {
        println!("org               {:<12} {}  tags: {}", o.org_id, o.name, o.tags.join(", "));
    }
    for c in &seed.carbons {
        let orgs: Vec<_> =
            c.memberships.iter().map(|(org, m)| format!("{org}:{}[{}]", m.org_role, m.tags.join(","))).collect();
        println!("carbon            @{:<12} {}  {}", c.carbon_id, c.name, orgs.join(" "));
    }
    for s in &seed.silicons {
        println!("silicon           @{:<12} {}  tags: {}  stk: {}", s.silicon_id, s.name, s.tags.join(", "), s.token);
    }
    let login =
        format!("http://{addr}/api/v1/login?app_id={}&redirect_uri=<yours>&org_id=tos", app.replace('>', "%3E"));
    println!("sign in           {login}  (add &as=<carbon> to skip the page)");
    server.await.map_err(io::Error::other)
}
