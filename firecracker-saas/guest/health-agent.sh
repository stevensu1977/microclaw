#!/bin/bash
# MicroClaw Firecracker microVM health check agent
# Runs inside the VM, periodically reports status to the host.

while true; do
  # 检查 MicroClaw 进程
  if pgrep -x microclaw > /dev/null; then
    STATUS="healthy"
  else
    STATUS="unhealthy"
  fi

  # 收集指标
  MEMORY=$(free -m | awk '/Mem:/ {print $3}')
  CPU=$(cat /proc/loadavg | awk '{print $1}')
  DISK=$(df -m /data 2>/dev/null | awk 'NR==2 {print $3}')
  UPTIME=$(awk '{print int($1)}' /proc/uptime)

  # 通过 HTTP 报告给 host (控制平面轮询此端口)
  echo "{\"status\":\"$STATUS\",\"memory_mb\":$MEMORY,\"load\":$CPU,\"disk_mb\":${DISK:-0},\"uptime_s\":$UPTIME}"

  sleep 10
done
