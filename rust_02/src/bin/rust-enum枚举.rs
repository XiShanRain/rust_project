#[derive(Debug)]
enum roadLevel {
    昨夜西风凋碧树,
    衣带渐宽终不悔,
    蓦然回首
}

fn main(){
    let level = roadLevel::昨夜西风凋碧树;
    println!("level-->{:?}", level);
}