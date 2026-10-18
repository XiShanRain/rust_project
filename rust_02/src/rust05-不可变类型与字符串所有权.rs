fn main(){
    let msg = "hello world".to_string(); //将字符串字面量转换为String类型
    let ref1 = &msg; //创建一个不可变引用ref1，指向msg
    let ref2 = ref1; //创建一个不可变引用ref2，指向ref1
    
    if ref1 == ref2 { //比较ref1和ref2的值，结果为true，因为它们指向同一个字符串
        println!("Both references point to the same string data.");
    }
}