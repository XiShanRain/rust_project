use std::io::Read;


fn main(){
    let file = std::fs::File::open("data.txt").unwrap();
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();
    println!("{}", content)
}