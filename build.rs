fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/tool-recap.ico");
        res.set("ProductName", "Tool Recap Rust");
        res.set("FileDescription", "AI video recap and voice automation studio");
        res.set("CompanyName", "Tool Recap");
        res.compile().expect("compile Windows resources");
    }
}
