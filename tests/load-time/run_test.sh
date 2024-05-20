#!/bin/bash

snapshots_directory=${1}
output_directory=${2:-"mri_output"}

if [ ! -d "$snapshots_directory" ]; then
    echo "Directory '$snapshots_directory' does not exist"
    exit 1
fi

echo "Run test without resource limitation"
for file in "$snapshots_directory"/*.snapshot; do
    if [ -f "$file" ]; then
        echo "Processing file: $file"
        echo ""
        sudo sh -c "sync; echo 3 > /proc/sys/vm/drop_caches"
        SNAPSHOT_PATH="$file" docker compose -f docker-compose-simple.yaml up
        docker rm -f -v $(docker ps -aq)

    fi
done
echo "Run test with IOPS limitation"
for file in "$snapshots_directory"/*.snapshot; do
    if [ -f "$file" ]; then
        echo "Processing file: $file"
        echo ""
        sudo sh -c "sync; echo 3 > /proc/sys/vm/drop_caches"
        SNAPSHOT_PATH="$file" docker compose -f docker-compose-iops.yaml up
        docker rm -f -v $(docker ps -aq)
    fi
done

echo "gather docker tests results"
echo "${output_directory}/simple/load_time.csv"
cat "${output_directory}/simple/load_time.csv"
echo "${output_directory}/limit_iops/load_time.csv"
cat "${output_directory}/limit_iops/load_time.csv"
tail "${output_directory}/simple/load_time.csv" >> "${output_directory}/load_time.csv"
tail -n +2 "${output_directory}/limit_iops/load_time.csv" >> "${output_directory}/load_time.csv"
sudo rm "${output_directory}/simple/load_time.csv" "${output_directory}/limit_iops/load_time.csv"

cp "${output_directory}/simple/*" "${output_directory}"
cp "${output_directory}/limit_iops/*" "${output_directory}"
echo "Remove docker tests folders"
sudo rm -rf "${output_directory}/simple" "${output_directory}/limit_iops"