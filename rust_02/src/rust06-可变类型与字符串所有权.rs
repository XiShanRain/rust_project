fn main(){
    let mut msg = "hello world".to_string(); //将字符串字面量转换为String类型,msg拥有所有权
    let ref1 = &mut msg; //创建一个可变引用ref1，指向msg
    let ref2 = ref1; //发生Move,ref1不再有效，ref2拥有msg的所有权
    
    if ref1 == ref2 { //提示: error[E0382]: borrow of moved value: `ref1`
        println!("Both references point to the same string data.");
    }
}