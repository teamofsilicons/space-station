#!/usr/bin/env python3
"""Emit the native API + separate ClickHouse CloudFormation stack. No containers."""
import json

ref = lambda name: {"Ref": name}
get = lambda name, attr: {"Fn::GetAtt": [name, attr]}
sub = lambda value: {"Fn::Sub": value}
tags = [{"Key": "Service", "Value": "space-station"}, {"Key": "Environment", "Value": "production"}]
resources = {}
def add(name, kind, props, retain=False):
    resources[name] = {"Type": kind, "Properties": props}
    if retain:
        resources[name].update(DeletionPolicy="Retain", UpdateReplacePolicy="Retain")

add("Artifacts", "AWS::S3::Bucket", {
    "BucketName": sub("space-station-production-${AWS::AccountId}-${AWS::Region}"),
    "BucketEncryption": {"ServerSideEncryptionConfiguration": [{"ServerSideEncryptionByDefault": {"SSEAlgorithm": "AES256"}}]},
    "PublicAccessBlockConfiguration": {k: True for k in ["BlockPublicAcls", "BlockPublicPolicy", "IgnorePublicAcls", "RestrictPublicBuckets"]},
    "VersioningConfiguration": {"Status": "Enabled"},
    "LifecycleConfiguration": {"Rules": [{"Id": "Backups", "Status": "Enabled", "Prefix": "backups/", "ExpirationInDays": 30, "NoncurrentVersionExpiration": {"NoncurrentDays": 7}}]},
    "Tags": tags,
}, True)
for name in ["ApiRuntime", "ClickhouseRuntime"]:
    add(name, "AWS::SecretsManager::Secret", {"Name": "space-station/production/" + name, "Tags": tags}, True)
for name, secret in [("Api", "ApiRuntime"), ("Clickhouse", "ClickhouseRuntime")]:
    add(name + "Role", "AWS::IAM::Role", {
        "AssumeRolePolicyDocument": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]},
        "ManagedPolicyArns": ["arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore", "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy"],
        "Policies": [{"PolicyName": "OwnRuntimeAndBackups", "PolicyDocument": {"Version": "2012-10-17", "Statement": [
            {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject"], "Resource": sub("${Artifacts.Arn}/*")},
            {"Effect": "Allow", "Action": "s3:ListBucket", "Resource": get("Artifacts", "Arn")},
            {"Effect": "Allow", "Action": "secretsmanager:GetSecretValue", "Resource": ref(secret)},
        ]}}], "Tags": tags,
    })
    add(name + "Profile", "AWS::IAM::InstanceProfile", {"Roles": [ref(name + "Role")]})
add("GatewaySecurityGroup", "AWS::EC2::SecurityGroup", {
    "GroupDescription": "Space Station HTTPS gateway", "VpcId": ref("VpcId"),
    "SecurityGroupIngress": [{"IpProtocol":"tcp", "FromPort":port, "ToPort":port, "CidrIp":"0.0.0.0/0"} for port in [80,443]], "Tags": tags,
})
add("Gateway", "AWS::ElasticLoadBalancingV2::LoadBalancer", {
    "Name":"space-station-production", "Type":"application", "Scheme":"internet-facing",
    "Subnets":["subnet-07945746462c26b2d", "subnet-0ba15539f08a1f48f"], "SecurityGroups":[ref("GatewaySecurityGroup")],
    "LoadBalancerAttributes":[{"Key":"idle_timeout.timeout_seconds","Value":"120"}, {"Key":"routing.http.drop_invalid_header_fields.enabled","Value":"true"}], "Tags":tags,
})
add("GatewayHttps", "AWS::ElasticLoadBalancingV2::Listener", {
    "LoadBalancerArn":ref("Gateway"), "Port":443, "Protocol":"HTTPS", "SslPolicy":"ELBSecurityPolicy-TLS13-1-2-2021-06",
    "Certificates":[{"CertificateArn":ref("CertificateArn")}], "DefaultActions":[{"Type":"forward","TargetGroupArn":ref("GatewayTargetGroup")}],
})
add("GatewayHttp", "AWS::ElasticLoadBalancingV2::Listener", {
    "LoadBalancerArn":ref("Gateway"), "Port":80, "Protocol":"HTTP",
    "DefaultActions":[{"Type":"redirect","RedirectConfig":{"Protocol":"HTTPS","Port":"443","StatusCode":"HTTP_301"}}],
})
metric=lambda name:{"CloudWatchMetricsEnabled":True,"SampledRequestsEnabled":True,"MetricName":"SpaceStation"+name}
add("GatewayFirewall", "AWS::WAFv2::WebACL", {
    "Name":"space-station-production", "Scope":"REGIONAL", "DefaultAction":{"Allow":{}}, "VisibilityConfig":metric("Gateway"),
    "Rules":[
        {"Name":"rate-limit", "Priority":0, "Action":{"Block":{}}, "Statement":{"RateBasedStatement":{"Limit":100000,"AggregateKeyType":"IP"}},"VisibilityConfig":metric("RateLimit")},
        {"Name":"ip-reputation", "Priority":1, "OverrideAction":{"None":{}}, "Statement":{"ManagedRuleGroupStatement":{"VendorName":"AWS","Name":"AWSManagedRulesAmazonIpReputationList"}},"VisibilityConfig":metric("IpReputation")},
        {"Name":"common", "Priority":2, "OverrideAction":{"None":{}}, "Statement":{"ManagedRuleGroupStatement":{"VendorName":"AWS","Name":"AWSManagedRulesCommonRuleSet","RuleActionOverrides":[{"Name":name,"ActionToUse":{"Count":{}}} for name in ["SizeRestrictions_BODY","EC2MetaDataSSRF_BODY","GenericLFI_BODY","GenericRFI_BODY","CrossSiteScripting_BODY"]]}}, "VisibilityConfig":metric("Common")},
    ], "Tags":tags,
})
add("GatewayFirewallAssociation", "AWS::WAFv2::WebACLAssociation", {"ResourceArn":ref("Gateway"),"WebACLArn":get("GatewayFirewall","Arn")})
add("ApiSecurityGroup", "AWS::EC2::SecurityGroup", {
    "GroupDescription": "Space Station API: only the production ALB may connect", "VpcId": ref("VpcId"),
    "SecurityGroupIngress": [{"IpProtocol": "tcp", "FromPort": 8080, "ToPort": 8080, "SourceSecurityGroupId": ref("GatewaySecurityGroup")}],
    "Tags": tags,
})
add("ClickhouseSecurityGroup", "AWS::EC2::SecurityGroup", {
    "GroupDescription": "ClickHouse: only the Space Station API may connect", "VpcId": ref("VpcId"),
    "SecurityGroupIngress": [{"IpProtocol": "tcp", "FromPort": 8123, "ToPort": 8123, "SourceSecurityGroupId": ref("ApiSecurityGroup")}],
    "Tags": tags,
})
bootstrap = """#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y curl ca-certificates unzip jq python3 python3-boto3 build-essential pkg-config libssl-dev
systemctl enable --now snap.amazon-ssm-agent.amazon-ssm-agent.service
install -d -m 0700 /etc/space-station
install -d -m 0755 /opt/space-station/releases /opt/space-station/bin
fallocate -l 4G /swapfile
chmod 600 /swapfile
mkswap /swapfile
swapon /swapfile
echo '/swapfile none swap sw 0 0' >> /etc/fstab
curl -fsSL https://awscli.amazonaws.com/awscli-exe-linux-aarch64.zip -o /tmp/awscli.zip
unzip -q /tmp/awscli.zip -d /tmp
/tmp/aws/install
rm -rf /tmp/aws /tmp/awscli.zip
"""
for name, size in [("Api", 64), ("Clickhouse", 128)]:
    add(name + "Instance", "AWS::EC2::Instance", {
        "ImageId": ref("AmiId"), "InstanceType": "t4g.large", "SubnetId": ref("SubnetId"),
        "SecurityGroupIds": [ref(name + "SecurityGroup")], "IamInstanceProfile": ref(name + "Profile"),
        "MetadataOptions": {"HttpTokens": "required", "HttpEndpoint": "enabled", "HttpPutResponseHopLimit": 1},
        "Monitoring": True,
        "BlockDeviceMappings": [{"DeviceName": "/dev/sda1", "Ebs": {"VolumeSize": size, "VolumeType": "gp3", "Encrypted": True, "DeleteOnTermination": False}}],
        "Tags": tags + [{"Key": "Name", "Value": "space-station-production-" + name.lower()}, {"Key": "Backup", "Value": "space-station"}],
        "UserData": {"Fn::Base64": bootstrap},
    }, True)
    add(name + "Recovery", "AWS::CloudWatch::Alarm", {
        "AlarmDescription": "Recover Space Station instance on EC2 system impairment", "Namespace": "AWS/EC2",
        "MetricName": "StatusCheckFailed_System", "Dimensions": [{"Name": "InstanceId", "Value": ref(name + "Instance")}],
        "Statistic": "Minimum", "Period": 60, "EvaluationPeriods": 2, "Threshold": 1,
        "ComparisonOperator": "GreaterThanOrEqualToThreshold", "AlarmActions": [sub("arn:aws:automate:${AWS::Region}:ec2:recover")],
    })
add("GatewayTargetGroup", "AWS::ElasticLoadBalancingV2::TargetGroup", {
    "Name": "space-station-native-api", "VpcId": ref("VpcId"), "Port": 8080, "Protocol": "HTTP", "TargetType": "instance",
    "Targets": [{"Id": ref("ApiInstance"), "Port": 8080}], "HealthCheckPath": "/api/health", "HealthCheckIntervalSeconds": 15,
    "HealthyThresholdCount": 2, "UnhealthyThresholdCount": 3, "Matcher": {"HttpCode": "200"},
    "TargetGroupAttributes": [{"Key": "deregistration_delay.timeout_seconds", "Value": "30"}], "Tags": tags,
})
add("SnapshotRole", "AWS::IAM::Role", {
    "AssumeRolePolicyDocument": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "dlm.amazonaws.com"}, "Action": "sts:AssumeRole"}]},
    "ManagedPolicyArns": ["arn:aws:iam::aws:policy/service-role/AWSDataLifecycleManagerServiceRole"],
})
add("DailySnapshots", "AWS::DLM::LifecyclePolicy", {
    "Description": "Daily Space Station snapshots retained seven days", "State": "ENABLED", "ExecutionRoleArn": get("SnapshotRole", "Arn"),
    "PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["INSTANCE"], "TargetTags": [{"Key": "Backup", "Value": "space-station"}],
        "Schedules": [{"Name": "Daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS", "Times": ["03:30"]}, "RetainRule": {"Count": 7}, "CopyTags": True}]},
})
for role, instance in [("api", "ApiInstance"), ("clickhouse", "ClickhouseInstance")]:
    add(role.title()+"Logs", "AWS::Logs::LogGroup", {"LogGroupName": "/space-station/production/"+role, "RetentionInDays": 30}, True)
    for metric, threshold, label in [("mem_used_percent", 90, "Memory"), ("disk_used_percent", 80, "Disk")]:
        add(role.title()+label+"Alarm", "AWS::CloudWatch::Alarm", {
            "AlarmDescription": "Space Station "+role+" "+label.lower()+" capacity",
            "Namespace": "SpaceStation", "MetricName": metric, "Dimensions": [{"Name": "InstanceId", "Value": ref(instance)}],
            "Statistic": "Average", "Period": 300, "EvaluationPeriods": 3, "Threshold": threshold,
            "ComparisonOperator": "GreaterThanThreshold", "TreatMissingData": "missing",
        })
    add(role.title()+"BackupAlarm", "AWS::CloudWatch::Alarm", {
        "AlarmDescription": "Space Station daily logical backup failed or missing",
        "Namespace": "SpaceStation", "MetricName": "BackupSuccess", "Dimensions": [{"Name": "Role", "Value": role}],
        "Statistic": "Minimum", "Period": 86400, "EvaluationPeriods": 1, "Threshold": 1,
        "ComparisonOperator": "LessThanThreshold", "TreatMissingData": "breaching",
    })
add("ApiHealthAlarm", "AWS::CloudWatch::Alarm", {
    "AlarmDescription": "Space Station API or dependency readiness is failing",
    "Namespace": "AWS/ApplicationELB", "MetricName": "UnHealthyHostCount",
    "Dimensions": [{"Name": "TargetGroup", "Value": get("GatewayTargetGroup", "TargetGroupFullName")}, {"Name": "LoadBalancer", "Value": get("Gateway", "LoadBalancerFullName")}],
    "Statistic": "Maximum", "Period": 60, "EvaluationPeriods": 3, "Threshold": 1,
    "ComparisonOperator": "GreaterThanOrEqualToThreshold", "TreatMissingData": "missing",
})
template = {"AWSTemplateFormatVersion": "2010-09-09", "Description": "Space Station: native Rust API, local PostgreSQL/Redis, separate native ClickHouse, private networking.",
    "Parameters": {k: {"Type": "String"} for k in ["VpcId", "SubnetId", "CertificateArn", "AmiId"]},
    "Resources": resources, "Outputs": {k: {"Value": v} for k, v in {
        "ApiInstanceId": ref("ApiInstance"), "ClickhouseInstanceId": ref("ClickhouseInstance"), "ClickhousePrivateIp": get("ClickhouseInstance", "PrivateIp"),
        "GatewayDns": get("Gateway","DNSName"), "GatewayArn": ref("Gateway"),
        "ArtifactBucket": ref("Artifacts"), "ApiRuntimeArn": ref("ApiRuntime"), "ClickhouseRuntimeArn": ref("ClickhouseRuntime"), "TargetGroupArn": ref("GatewayTargetGroup"),
    }.items()}}
print(json.dumps(template, indent=2))
