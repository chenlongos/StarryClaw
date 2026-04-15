fn main() {
    cc::Build::new()
        .file("src/c_http_client.c")
        .flag_if_supported("-std=c11")
        .compile("sc_http_client");
}
