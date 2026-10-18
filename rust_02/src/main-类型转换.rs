struct Point {
    x: i32,
    y: i32,
}

struct Circle {
    radius: f32,
    angle: f32,
}

fn print_shape_points(point: &Point) {
    println!("Points: (x={}, y={})", point.x, point.y);
}

fn print_shape_circle(circle: &Circle) {
    println!("Circle: (radius={:.2}, angle={:.2})", circle.radius, circle.angle);
}

fn main() {
    let point = Point { x: 10, y: 20 };
    let circle = Circle { radius: 5.0, angle: 45.0 };

    //Rust不允许这样：
    // print_shape_circle(&point); // 错误：无法将Point转换为Circle  expected `&Circle`, found `&Point`
    print_shape_circle(&point as &Circle);

    // 由于函数重载的存在，Rust会根据传入参数的类型来决定调用哪个函数。
    // print_shape_points(&point);
    // print_shape_circle(&circle);    
}
