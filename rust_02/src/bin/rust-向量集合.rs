use std::collections::HashMap;

fn main(){
    //1. 定义向量
    let mut vec1 = Vec::new();
    vec1.push("午梦千山，窗阴一箭。");
    vec1.push("水精双枕，傍有堕钗横。");
    println!("{:?}", vec1);

    let mut vec2 = vec!["悟空","悟能", "悟净"];
    println!("{:?}", vec2);

    let x = vec2.remove(0);
    println!("{:?}", vec2);

    let y = vec2[0];
    println!("{}", y);

    for item in vec1 {
        println!("{}", item);
    }

    //2. HashMap 键值对集合
    let mut map1 = HashMap::new();
    map1.insert(1, "孙悟空");
    map1.insert(2, "猪八戒");
    map1.insert(3,"沙悟净");
    println!("{:?}", map1);

    for item in map1 {
        println!("{:?}", item);
    }
    println!("{:?}", map1); //value borrowed here after move

    let mut map2 = HashMap::new();
    map2.insert(1, "刘备");
    map2.insert(2, "关羽");
    map2.insert(3,"张飞");
    for (k, v) in map2.iter() {
        println!("{}-->{}", k, v);
    }


    let x = map2.get(&2); // 传入 &i32 类型的键
    println!("{:?}", x); // 输出: Some("关羽")

    match map2.get(&1) {
        None => {
            println!("找不到");
        }
        Some(data) => {
            println!("找到了: {:?}", data)
        }
    }

}