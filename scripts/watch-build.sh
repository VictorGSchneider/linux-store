#!/bin/sh
# Loop de monitoramento. Roda um comando em segundo plano e grava só o essencial:
#   <dir>/out.log   saída completa (o monitor NÃO a lê)
#   <dir>/exit      código de saída, criado só quando o comando termina
# Uso:   scripts/watch-build.sh start <dir> <comando...>
#        scripts/watch-build.sh wait  <dir> [intervalo_s=60]   (imprime só "ok" ou "falhou <código>")
set -u
case "${1:-}" in
start)
  dir=$2; shift 2; mkdir -p "$dir"; rm -f "$dir/exit"
  ( "$@" >"$dir/out.log" 2>&1; echo $? >"$dir/exit.tmp"; mv "$dir/exit.tmp" "$dir/exit" ) &
  echo "iniciado em $dir" ;;
wait)
  dir=$2; every=${3:-60}
  until [ -f "$dir/exit" ]; do sleep "$every"; done
  code=$(cat "$dir/exit"); [ "$code" = 0 ] && echo ok || echo "falhou $code" ;;
*) echo "uso: $0 start <dir> <comando...> | wait <dir> [s]" >&2; exit 2 ;;
esac
