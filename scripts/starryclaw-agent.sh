#!/bin/sh
# StarryClaw — POSIX shell version for StarryOS (/bin/sh) without Rust binary.
# Same commands as the Rust starryclaw: 查询 / 创建 / ls / mkdir

echo "StarryClaw sh 兜底（无模型）。输入 quit 退出。"
while printf "StarryClaw · sh › " && IFS= read -r line; do
	case $line in
		quit|exit) break ;;
	esac

	# trim leading spaces (basic)
	line=$(echo "$line" | sed 's/^[[:space:]]*//')

	case $line in
		查询*)
			path=$(echo "$line" | sed 's/^查询[[:space:]]*//')
			test -z "$path" && path=.
			ls -la "$path"
			echo "[ok]"
			;;
		ls|ls\ *)
			path=$(echo "$line" | sed 's/^ls[[:space:]]*//')
			test -z "$path" && path=.
			ls -la "$path"
			echo "[ok]"
			;;
		list|list\ *)
			path=$(echo "$line" | sed 's/^list[[:space:]]*//')
			test -z "$path" && path=.
			ls -la "$path"
			echo "[ok]"
			;;
		创建*)
			name=$(echo "$line" | sed 's/^创建[[:space:]]*//')
			if test -z "$name"; then
				echo "[error] missing directory name"
			elif echo "$name" | grep -q '[./]'; then
				echo "[error] only a single name, no slashes"
			else
				mkdir -p "$name"
				echo "[ok]"
			fi
			;;
		mkdir\ *)
			name=$(echo "$line" | sed 's/^mkdir[[:space:]]*//')
			if test -z "$name"; then
				echo "[error] missing directory name"
			elif echo "$name" | grep -q '[./]'; then
				echo "[error] only a single name, no slashes"
			else
				mkdir -p "$name"
				echo "[ok]"
			fi
			;;
		cd|cd\ *)
			path=$(echo "$line" | sed 's/^cd[[:space:]]*//')
			test -z "$path" && path="$HOME"
			if cd "$path" 2>/dev/null; then pwd; echo "[ok]"; else echo "[error] cd failed"; fi
			;;
		cat\ *)
			f=$(echo "$line" | sed 's/^cat[[:space:]]*//')
			if test -z "$f"; then
				echo "[error] missing file"
			else
				cat "$f"
				echo "[ok]"
			fi
			;;
		*)
			echo "未知指令。试试：查询、创建、cd、cat、ls、mkdir"
			;;
	esac
done
