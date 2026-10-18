fn main() {
    // 模拟实际业务：订单支付、退款、物流查询的结果
    let paid_order: Option<String> = Some("ORD-20260109-8888".to_string());
    let refund_result: Option<f64> = None;      // 退款失败，无退款金额
    let logistics_info: Option<String> = None;  // 暂无物流信息

    // 1. 支付成功处理：只需要处理成功的情况
    //    （用 match 会被迫写一个多余的 None 分支）
    if let Some(order_no) = &paid_order {
        println!("订单 {} 支付成功，正在发货...", order_no);
    }

    // 2. 退款处理：需要指明失败情形
    if let Some(amount) = refund_result {
        println!("退款成功，退款金额：{:.2} 元", amount);
    } else {
        // 解构失败，退款申请未通过
        println!("退款失败，请稍后重试或联系客服。");
    }

    // 3. 物流查询：提供多种备选分支
    let use_cache = true; // 是否尝试读取缓存的物流数据
    if let Some(express_no) = logistics_info {
        println!("物流单号：{}，可点击查询详情", express_no);
    } else if use_cache {
        // 没查到实时物流，尝试从缓存读取
        println!("实时物流暂未更新，正在为您展示缓存数据...");
    } else {
        // 默认分支
        println!("暂无物流信息，请稍后再试。");
    }
}
