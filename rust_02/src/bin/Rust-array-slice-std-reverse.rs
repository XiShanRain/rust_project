fn main(){
    //测试反转数组
    let mut arr = [1, 2, 3, 4, 5];
    arr.reverse();
    println!("Reversed array: {:?}", arr);

    let mut arr = [1, 2, 3, 4, 5];
    let s1 = &mut arr;
    s1.reverse();
    println!("Reversed array again: {:?}", s1);

    let s2 = &mut arr[1..4];  // 切片覆盖 [2, 3, 4]
    s2.reverse();  // 只反转切片部分，结果 arr = [1, 4, 3, 2, 5]
    println!("Array with reversed slice: {:?}", arr);
}