fn foo(v1: &Vec<i32>, v2: &Vec<i32>) -> i32 {
    println!("{:?}", v1);
    println!("{:?}", v2);

    42
}
fn main() {
    let v1 = vec![1,2,3,4,5];
    let v2 = vec![6,7,8,9,10];
    let ret = foo(&v1, &v2);
    println!("ret is {}", ret);
}
