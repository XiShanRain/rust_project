fn receive_closure<T>(f: T)
where
    T: Fn(i32,i32) -> i32,
{
    let result = f(10, 20);
    println!("Result from closure: {}", result);
}

fn receive_closure_with_outer_variable<T>(f: T)
where
    T: Fn(i32,i32) -> i32,
{
    
    let result = f(1, 10);
    println!("Result from closure with outer variable: {}", result);
}


fn main() {
    let add = |x, y| x + y;
    let sum = add(10, 20);
    println!("Sum: {}", sum);

    receive_closure(add);

    let multiply = |x, y| x * y;
    receive_closure(multiply);

    let v = 3;
    receive_closure_with_outer_variable(|x, y| x + y + v);
}
