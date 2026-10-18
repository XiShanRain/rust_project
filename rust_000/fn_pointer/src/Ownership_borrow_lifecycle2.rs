
fn skip_prefix<'a, 'b>(line: &'a str, prefix: &'b str) -> &'a str {
    line
}


fn main() {
    let line = "人似秋鸿来有信";
    let lang = "事如春梦了无痕";

    let v;
    {
        let p = format!("苏轼:{}=", lang);
        v = skip_prefix(line, p.as_str());
    }
    println!("v is {}", v);
}