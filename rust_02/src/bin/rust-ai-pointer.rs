fn main(){
    let x = 5;
    let y  = Box::new(x);
    println!("{}", 5 == x);
    println!("{}", 5 == *y);
}