#!/bin/bash
# 创建 TAP 网络设备 for Firecracker microVM
#
# 用法: ./create-tap.sh <tenant_id> <gateway_ip>
# 示例: ./create-tap.sh acme-corp 172.16.42.1

set -e

TENANT_ID="$1"
GATEWAY_IP="$2"

if [ -z "$TENANT_ID" ] || [ -z "$GATEWAY_IP" ]; then
  echo "Usage: $0 <tenant_id> <gateway_ip>"
  echo "Example: $0 acme-corp 172.16.42.1"
  exit 1
fi

# TAP 设备名: 限制 15 字符 (Linux 网络接口名最大长度)
TAP_NAME="fc-${TENANT_ID:0:11}"

echo "==> Creating TAP device: $TAP_NAME (gateway=$GATEWAY_IP)"

# 创建 TAP 设备
ip tuntap add dev "$TAP_NAME" mode tap
ip addr add "$GATEWAY_IP/30" dev "$TAP_NAME"
ip link set "$TAP_NAME" up

# 启用 IP 转发
sysctl -w net.ipv4.ip_forward=1 > /dev/null

# 检测主机出口网卡
HOST_IFACE=$(ip route get 8.8.8.8 | awk '{print $5; exit}')

# 启用 NAT (允许 VM 访问外网)
iptables -t nat -A POSTROUTING -s "${GATEWAY_IP%.*}.0/30" -o "$HOST_IFACE" -j MASQUERADE
iptables -A FORWARD -i "$TAP_NAME" -o "$HOST_IFACE" -j ACCEPT
iptables -A FORWARD -i "$HOST_IFACE" -o "$TAP_NAME" -m state --state RELATED,ESTABLISHED -j ACCEPT

# 阻止租户间通信
# 注意: 这条规则应该只添加一次，多次调用不会重复
iptables -C FORWARD -i "fc-+" -o "fc-+" -j DROP 2>/dev/null || \
  iptables -A FORWARD -i "fc-+" -o "fc-+" -j DROP

echo "==> TAP device $TAP_NAME created successfully"
echo "    Host gateway: $GATEWAY_IP/30"
echo "    VM IP:        ${GATEWAY_IP%.*}.$((${GATEWAY_IP##*.} + 1))/30"
