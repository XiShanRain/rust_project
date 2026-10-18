fn foo(v: &Vec<i32>) {
    v.push(100);
}
fn main() {
    let v1 = vec![];
    foo(&v1);
}
