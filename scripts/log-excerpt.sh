#!/bin/sh
# Loop de correção: imprime só o trecho do log com o erro, nunca o log inteiro.
# Uso: scripts/log-excerpt.sh <out.log> [linhas_de_contexto=25]
# Procura o primeiro "error"/"FAILED"/"panicked" e mostra a partir dali; se não achar, as últimas 15 linhas.
log=$1; ctx=${2:-25}
n=$(grep -nE '^(error|warning: unused)|FAILED|panicked at|^failures:' "$log" | head -1 | cut -d: -f1)
if [ -n "$n" ]; then sed -n "${n},$((n + ctx))p" "$log"; else tail -n 15 "$log"; fi
