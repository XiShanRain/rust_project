fn main(){
    fn get_item_safe(index: usize) -> Option<i32> {
        let vec = vec![1, 2, 3];
        if index < vec.len() {
                Some(vec[index])  // 明确表示"有值"
        } else {
            None              // 明确表示"无值"
        }
    }

    // 编译器强制调用者处理两种情况
    match get_item_safe(5) {
        Some(value) => println!("{}", value),
        None => println!("索引越界！"),  // 必须处理
    }
}