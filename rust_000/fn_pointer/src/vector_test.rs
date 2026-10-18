fn main(){
    let v = vec![1,2,3,4,5];
    match v.get(10) {
        Some(x) => println!("x = {}", x),
        None => println!("None"),
    }

    let mut v = vec![1,2,3,4,5];
    println!("Reference to the vector:");
    for i in &v {
        println!("A reference to {}", i);
    }

    println!("Mutable reference to the vector:");
    for i in &mut v {
        println!("A mutable reference to {}", i);
    }

    println!("Take ownership of the vector and its element:");
    for i in v {
        println!("Take ownership of the vector and its element {}", i);
    }
    

}