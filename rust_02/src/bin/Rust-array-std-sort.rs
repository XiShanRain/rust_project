fn main() {
    let mut my_array = [5, 9, 6, 4, 2];
    println!("Original array: {:?}", my_array);
    my_array.sort();
    println!("Sorted array: {:?}", my_array);

    //测试concat
    let arr1 = [1, 2, 3];
    let arr2 = [4, 5, 6];
    let arr3 = [arr1.as_slice(), arr2.as_slice()].concat();
    println!("Concatenated array: {:?}", arr3);

    let arr4 = [arr1, arr2].concat();
    println!("Concatenated array again: {:?}", arr4);

    let my_array2 = [34, 56, 7, 89, 5, 4, 3, 6, 73, 2];
    let (l, r) = my_array2.split_at(2);
    println!("{:?}", l);
    println!("{:?}", r);
}
