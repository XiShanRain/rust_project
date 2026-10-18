fn main() {
    //1. 计数器
    let mut counter = 0;
    counter += 1;
    counter += 1;
    counter += 1;

    println!("{}", counter);

    //2. 累加器
    let mut sum = 0;
    for i in 1..=100 {
        sum += i;
    }
    println!("{}", sum);

    // 3. 状态管理
    let is_connected = connect_to_server();
    if is_connected {
        println!("Connected to server!");
    } else {
        println!("Failed to connect to server.");
    }

    //4. 数据结构修改
    let mut vec = vec![1, 2, 3];
    vec.push(4);
    vec[0] = 10;
    println!("向量: {:?}", vec);
}

fn connect_to_server() -> bool {
    true
}