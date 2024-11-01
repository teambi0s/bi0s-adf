#!/bin/bash

generate_ns_name() {
    local ip_address=$1
    local last_octet=$(echo $ip_address | cut -d. -f4)
    echo "ns${last_octet}"
}

validate_ip() {
    local ip=$1
    
    local subnet=$(echo $ip | cut -d. -f1-3)
    if [ "$subnet" != "10.42.0" ]; then
        echo "Error: IP must be in the 10.42.0.0/24 subnet"
        return 1
    fi
    
    local last_octet=$(echo $ip | cut -d. -f4)
    if [ $last_octet -lt 2 ] || [ $last_octet -gt 254 ]; then
        echo "Error: Last octet must be between 2 and 254"
        return 1
    fi
    return 0
}

create_namespace() {
    local ip_address=$1
    local ns_name=$(generate_ns_name $ip_address)
    
    echo "Creating namespace: $ns_name"
    
    sudo sysctl -w net.ipv4.ip_forward=1 > /dev/null
    
    sudo ip netns add $ns_name
    sudo ip link add veth-$ns_name type veth peer name veth-$ns_name-host
    sudo ip link set veth-$ns_name netns $ns_name
    sudo ip netns exec $ns_name ip addr add $ip_address/24 dev veth-$ns_name
    sudo ip netns exec $ns_name ip link set veth-$ns_name up
    sudo ip netns exec $ns_name ip route add default via 10.42.0.1
    sudo ip addr add 10.42.0.1/24 dev veth-$ns_name-host
    sudo ip link set veth-$ns_name-host up
    
    sudo iptables -t nat -A POSTROUTING -s 10.42.0.0/24 -j MASQUERADE
    
    echo -e "\nTo open a shell in the namespace, run:"
    echo "sudo ip netns exec $ns_name bash"
}

destroy_namespace() {
    local ip_address=$1
    local ns_name=$(generate_ns_name $ip_address)
    
    echo "Cleaning up namespace: $ns_name"
    
    sudo iptables -t nat -D POSTROUTING -s 10.42.0.0/24 -j MASQUERADE 2>/dev/null
    sudo ip link delete veth-$ns_name-host 2>/dev/null
    sudo ip netns del $ns_name 2>/dev/null
    
    echo "Cleanup completed"
}

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <source_ip> <create|destroy>"
    exit 1
fi

SOURCE_IP=$1
ACTION=$2

if ! validate_ip $SOURCE_IP; then
    exit 1
fi

case $ACTION in
    create)
        create_namespace $SOURCE_IP
        ;;
    destroy)
        destroy_namespace $SOURCE_IP
        ;;
    *)
        echo "Invalid action. Use 'create' or 'destroy'"
        exit 1
        ;;
esac
