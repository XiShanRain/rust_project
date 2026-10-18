fn main() {
    //不可变变量
     let data = vec![1, 2, 3];
     let ref1 = &data; // 不可变引用
     let ref2 = &data; // 不可变引用
     let ref3 = &data; // 不可变引用
     println!("不可变引用: {:?}, {:?}, {:?}", ref1, ref2, ref3);
     println!("不可变引用: {:p}, {:p}, {:p}", ref1, ref2, ref3);
    
    // let mut ref4 = &data; // 不可变引用
    // println!("可变引用: {:?}", ref4);
    // ref4.push(4);
    // println!("可变引用: {:?}", ref4);

     //可变变量
    //  let mut mut_data = vec![1, 2, 3];
    //  let mut_ref1 = &mut mut_data; // 第一次借用
    //  let mut_ref2 = &mut mut_data; // 第二次借用 报异常: cannot borrow `mut_data` as mutable more than once at a time
    //  println!("可变引用: {:?}, {:?}", mut_ref1, mut_ref2);

    // let mut mut_data = vec![1, 2, 3];

    // let mut_ref1 = &mut mut_data;
    // mut_ref1.push(4);           // mut_ref1 最后一次使用
    // println!("可变引用: {:?}", mut_ref1);

    // let mut_ref2 = &mut mut_data; // ✅ 编译通过！
    // mut_ref2.push(5);             // mut_ref1 已"死亡"，不再冲突

    // let mut mut_data = vec![1, 2, 3];

    // let mut_ref1 = &mut mut_data; // 第一次借用
    // let mut_ref2 = &mut mut_data; // ❌ 报错！因为 mut_ref1 之后还要用，
    //                             //    两个可变引用同时活跃

    // mut_ref1.push(4);             // mut_ref1 在借出后仍被使用
    // mut_ref2.push(5);

    // 假设这段代码被允许
    let mut v = vec![1, 2, 3];
    let r1 = &mut v;
    let r2 = &mut v;

    r1.push(4);          // 扩容导致堆内存重新分配，v 内部的指针更新了
                        // 但 r2 可能还持有旧的地址
    r2.push(5);          // 通过失效的指针操作 → 内存不安全！

}
