// Redirecting to a new URL with the res.redirect function
mod common;

use common::{request, serve};
use cpeak::cpeak;

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();

        server.route("get", "/old-route", async |_req, res| {
            res.redirect("/new-route")
        });

        server
    })
}

#[test]
fn should_redirect_to_the_new_route() {
    let res = request(server(), "GET", "/old-route").send();
    assert_eq!(res.status, 302);
    assert_eq!(res.header("location"), Some("/new-route"));
}
