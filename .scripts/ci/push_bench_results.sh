#!/bin/bash
#The script takes output.txt, removes every line that doesn't have "test"
#in it and pushes benchmark result to Victoria Metrics
#Benchmark name should have underscores in name (e.g. async/http_concurrent_round_trip/8)

RESULT_FILE=$1
CURRENT_DIR=$(pwd)

if [ -z "$RESULT_FILE" ]
then
  RESULT_FILE="output.txt"
fi

cat $RESULT_FILE | grep test > $CURRENT_DIR/output_redacted.txt

INPUT="output_redacted.txt"

while IFS= read -r line
do
  BENCH_NAME=$(echo $line | cut -f 2 -d ' ')
  BENCH_RESULT=$(echo $line | cut -f 5 -d ' ')
  curl -d 'benchmark{project="'${CI_PROJECT_NAME}'",metric="'$BENCH_NAME'"} '$BENCH_RESULT'' \
    -X POST 'http://vm-longterm.parity-build.parity.io/api/v1/import/prometheus'
done < "$INPUT"
